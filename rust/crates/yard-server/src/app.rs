//! HTTP routes.

use crate::history::History;
use crate::palette;
use crate::sessions::{Sessions, StepError};
use crate::store::Store;
use crate::sync::{free_name, LinkError, Sync, SyncError};
use anyhow::Context;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use yard_core::api::{
    ApiError as ErrorBody, ClientBlock, GameReport, NewDesign, StepRequest, StepResponse, Summary, SyncDone, SyncInfo, SyncPlan, SyncStatus,
    SyncSwitch, PRINT_HEADER, REV_HEADER, TOKEN_HEADER,
};
use yard_core::api::{GameYard, SyncVersion, YardGiven, YardLinkInfo, YardLinkRequest, YardTrouble};
use yard_core::blueprint::{game_name, valid_name, MAX_NAME};
use yard_core::catalog::Catalog;
use yard_core::Blueprint;

/// Largest upload accepted. Big ships are a few tens of megabytes.
const MAX_UPLOAD: usize = 256 * 1024 * 1024;

pub struct Config {
    /// A StarMade install; block definitions and textures are read from it.
    pub starmade_dir: PathBuf,
    pub data_dir: PathBuf,
    /// The built browser client.
    pub static_dir: PathBuf,
    /// If set, a game server's sync agent has to present this secret.
    pub sync_token: Option<String>,
    /// Seconds a design has to be left alone before it is sent to the game.
    pub sync_quiet_secs: u64,
}

pub struct AppState {
    pub(crate) catalog: Catalog,
    /// The catalog as the client needs it, serialised once.
    catalog_json: Bytes,
    pub(crate) store: Store,
    pub(crate) sessions: Sessions,
    /// The exchange with a game server's blueprint catalog.
    pub(crate) sync: Sync,
    /// The versions of each design that were in step with the game.
    pub(crate) history: History,
    sync_token: Option<String>,
    static_dir: PathBuf,
    /// The install's game build, as blueprint headers spell it.
    game_version: String,
    /// The game's block texture sheets (t000.png, t001.png...).
    texture_dir: PathBuf,
    /// The game's 3D block models and their texture sheets.
    models: crate::models::Models,
    /// The hull blocks 3D models are made of, with their colours.
    hull_palette: Vec<(u16, [u8; 3])>,
    /// How models are reached, and the chat of each design.
    pub(crate) chat_config: crate::chat::ChatConfig,
    pub(crate) chats: crate::chat::Chats,
}

impl AppState {
    pub async fn load(config: Config) -> anyhow::Result<Arc<Self>> {
        let cfg = config.starmade_dir.join("data/config");
        let xml = std::fs::read_to_string(cfg.join("BlockConfig.xml")).with_context(|| {
            format!(
                "cannot read {}. Set YARD_STARMADE_DIR to a StarMade install.",
                cfg.join("BlockConfig.xml").display()
            )
        })?;
        let props = std::fs::read_to_string(cfg.join("BlockTypes.properties"))
            .with_context(|| format!("cannot read {}", cfg.join("BlockTypes.properties").display()))?;
        let catalog = Catalog::parse(&xml, &props)?;
        let colours = palette::load(&config.starmade_dir, &catalog);
        // Model textures take the page numbers after the block texture sheets.
        let block_pages = catalog.all().flat_map(|d| d.textures).max().map_or(0, |highest| highest.max(0) / 256 + 1);
        let models = crate::models::load(&config.starmade_dir, &catalog, block_pages as u16);
        tracing::info!("{} block models", models.len());
        tracing::info!("{} block types, {} with texture colours", catalog.len(), colours.len());

        let blocks: Vec<ClientBlock> = catalog
            .all()
            .map(|d| {
                let mut block = ClientBlock::of(d, colours.get(&d.id).copied().unwrap_or(palette::FALLBACK));
                block.model = models.index(&d.model);
                block.model_on = models.index(&d.model_on);
                block
            })
            .collect();
        let catalog_json = Bytes::from(serde_json::to_vec(&blocks)?);
        let hull_palette = crate::importer::hull_palette(&catalog, &colours);

        let store = Store::open(&config.data_dir.join("designs"))?;
        let version_txt = std::fs::read_to_string(config.starmade_dir.join("version.txt")).unwrap_or_default();
        let game_version = yard_core::blueprint::header_game_version(&version_txt);
        tracing::info!("new designs are marked as game version {game_version}");
        let sync = Sync::open(&config.data_dir, config.sync_quiet_secs)?;
        let history = History::open(&config.data_dir);
        history.sweep();
        sync.seed_history(&store, &history);
        Ok(Arc::new(Self {
            catalog,
            catalog_json,
            store,
            sessions: Sessions::default(),
            sync,
            history,
            sync_token: config.sync_token.filter(|t| !t.is_empty()),
            static_dir: config.static_dir,
            game_version,
            texture_dir: palette::texture_dir(&config.starmade_dir),
            models,
            hull_palette,
            chat_config: crate::chat::ChatConfig::from_env(),
            chats: Default::default(),
        }))
    }

    /// Writes edited designs to disk. See [`Sessions::flush`]. Blocking.
    pub fn flush(&self, everything: bool) {
        self.sessions.flush(&self.store, &self.catalog, everything);
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    // Client-side routes such as /d/<id> all load the same page.
    let index = ServeFile::new(state.static_dir.join("index.html"));
    let files = ServeDir::new(&state.static_dir).fallback(index);
    Router::new()
        .route("/api/catalog", get(catalog))
        .route("/api/models", get(models))
        .route("/api/textures/{page}", get(texture_page))
        .route("/api/designs", get(list_designs).post(upload_design))
        .route("/api/designs/new", post(new_design))
        .route("/api/chat/models", get(chat_models))
        .route("/api/designs/{id}/chat", post(chat_ask).get(chat_log).delete(chat_reset))
        .route("/api/designs/{id}/chat/stop", post(chat_stop))
        .route("/api/import/formats", get(import_formats))
        .route("/api/designs/import", post(import_model))
        .route("/api/designs/{id}", get(get_design).delete(delete_design))
        .route("/api/designs/{id}/file", get(get_design_file))
        .route("/api/designs/{id}/steps", post(step))
        .route("/api/designs/{id}/relocate-core", post(relocate_core))
        .route("/api/designs/{id}/move-rail", post(move_rail))
        .route("/api/designs/{id}/fit-pivots", post(fit_pivots))
        .route("/api/designs/{id}/clone-docked", post(clone_docked))
        .route("/api/designs/{id}/make-turret", post(make_turret))
        .route("/api/designs/{id}/restore", post(restore_structure))
        .route("/api/designs/{id}/sync", post(switch_sync))
        .route("/api/designs/{id}/sync/now", post(sync_now))
        .route("/api/designs/{id}/yard", get(yard_link).post(link_yard).delete(unlink_yard))
        .route("/api/sync/yards", get(sync_yards))
        .route("/api/sync/yards/take", put(yard_take))
        .route("/api/sync/yards/given", post(yard_given))
        .route("/api/sync/yards/trouble", post(yard_trouble))
        .route("/api/designs/{id}/rename", post(rename_design))
        .route("/api/designs/{id}/history", get(sync_history))
        .route("/api/designs/{id}/history/{seq}/file", get(version_file))
        .route("/api/designs/{id}/history/{seq}/restore", post(restore_version))
        .route("/api/designs/{id}/history/{seq}/copy", post(copy_version))
        .route("/api/sync", get(sync_info))
        .route("/api/sync/report", post(sync_report))
        .route("/api/sync/blueprints", put(sync_receive))
        .route("/api/sync/designs/{id}/file", get(sync_file))
        .route("/api/sync/done", post(sync_done))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
        .fallback_service(files)
        .layer(CompressionLayer::new())
        .with_state(state)
}

/// An error the client can show: a status and a sentence.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorBody { error: self.1 })).into_response()
    }
}

async fn catalog(State(state): State<Arc<AppState>>) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], state.catalog_json.clone()).into_response()
}

/// One of the game's block texture sheets, straight from the install. The
/// textures belong to the game, so they are served from there, not shipped here.
async fn texture_page(State(state): State<Arc<AppState>>, UrlPath(page): UrlPath<u32>) -> Result<Response, ApiError> {
    let missing = || ApiError(StatusCode::NOT_FOUND, "No such texture sheet.".into());
    let cached = [(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")];
    // The pages after the block textures are the textures of the 3D models.
    if state.models.has_sheet(page) {
        let sheet = blocking(move || state.models.sheet(page).ok_or_else(missing)).await?;
        return Ok((cached, sheet).into_response());
    }
    if page > 99 {
        return Err(missing());
    }
    let bytes = tokio::fs::read(state.texture_dir.join(format!("t{page:03}.png"))).await.map_err(|_| missing())?;
    Ok((cached, bytes).into_response())
}

/// The 3D models the game draws for some blocks in place of a cube.
async fn models(State(state): State<Arc<AppState>>) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=3600")], state.models.json.clone()).into_response()
}

/// Every design. Editors ask again every few seconds to see what a game
/// server brought in, so an unchanged list is answered with "not modified"
/// instead of being sent again: with a game's whole catalog it is not small.
async fn list_designs(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    use std::hash::{Hash, Hasher};
    let mut all = state.store.list();
    state.sync.mark(&mut all);
    // A design with edits still on their way to disk is no longer what the
    // game has, whatever the stored copy says.
    let unsaved = state.sessions.all_unsaved();
    for summary in all.iter_mut().filter(|s| unsaved.contains(&s.id)) {
        if summary.sync == Some(SyncStatus::Synced) {
            summary.sync = Some(SyncStatus::Waiting);
        }
    }
    let body = serde_json::to_vec(&all).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    let tag = format!("\"{:016x}\"", hasher.finish());
    let unchanged = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(tag.as_str());
    let caching = [(header::ETAG, tag), (header::CACHE_CONTROL, "no-cache".to_string())];
    Ok(if unchanged {
        (StatusCode::NOT_MODIFIED, caching).into_response()
    } else {
        (caching, [(header::CONTENT_TYPE, "application/json")], body).into_response()
    })
}

/// A design's summary with its standing with the game server filled in.
fn marked(state: &AppState, summary: Summary) -> Summary {
    let mut one = [summary];
    state.sync.mark(&mut one);
    let [mut summary] = one;
    if summary.sync == Some(SyncStatus::Synced) && state.sessions.unsaved(&summary.id) {
        summary.sync = Some(SyncStatus::Waiting);
    }
    summary
}

async fn get_design(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Json<Summary>, ApiError> {
    let summary = state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
    Ok(Json(marked(&state, summary)))
}

/// Deletes a design. Its files are set aside in the data folder's `trash`, not
/// erased. A blueprint the game has for it stays in the game, and is not
/// brought back here unless it changes there.
async fn delete_design(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<StatusCode, ApiError> {
    blocking(move || {
        let summary = state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
        let when = unix_now();
        state.chats.stop(&id);
        let broken = |e: anyhow::Error| {
            tracing::error!("deleting design {id}: {e:#}");
            ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The design could not be deleted.".into())
        };
        // Closed first: edits not yet on disk are written, then the files move.
        state.sessions.replacing(&state.store, &state.catalog, &id, || state.store.trash(&id, when)).map_err(broken)?;
        if let Err(e) = state.history.set_aside(&id, &state.store.trash_dir().join(format!("{id}-{when}-history"))) {
            tracing::error!("setting aside the sync history of design {id}: {e:#}");
        }
        state.sync.forget(&id);
        tracing::info!("deleted design {id} \"{}\"", summary.name);
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Gives a design a new name. The name follows the game's rule for blueprint
/// names and no other design may have it. A design that was in the game goes
/// there afresh under its new name; the blueprint under the old name stays in
/// the game.
async fn rename_design(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<yard_core::api::Rename>,
) -> Result<Json<Summary>, ApiError> {
    let name = game_name(&request.name);
    if name.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Give the design a name: letters, digits, spaces, - and _.".into()));
    }
    blocking(move || {
        let summary = state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
        if summary.name == name {
            return Ok(Json(marked(&state, summary)));
        }
        if state.store.list().iter().any(|s| s.id != id && s.name.to_lowercase() == name.to_lowercase()) {
            return Err(ApiError(StatusCode::CONFLICT, format!("Another design is called \"{name}\" already.")));
        }
        // The game tells names apart by more than capitals: the blueprint would keep the old spelling.
        if summary.name.to_lowercase() == name.to_lowercase() && state.sync.in_game(&id) {
            return Err(ApiError(StatusCode::CONFLICT, "The game does not tell names apart by capitals alone, so a design in the game cannot be renamed that way.".into()));
        }
        state.sessions.rename(&state.store, &state.catalog, &id, &name)?;
        let was_in_game = state.sync.renamed(&id);
        tracing::info!("renamed design {id} \"{}\" to \"{name}\"{}", summary.name, if was_in_game { "; it goes to the game afresh" } else { "" });
        let summary = state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
        Ok(Json(marked(&state, summary)))
    })
    .await
}

/// Runs blocking work (parsing, compressing, big edits) off the async threads.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work).await.map_err(|e| {
        tracing::error!("worker task failed: {e}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The server could not finish that request.".into())
    })?
}

impl From<StepError> for ApiError {
    fn from(e: StepError) -> Self {
        match e {
            StepError::NoSuchDesign => ApiError(StatusCode::NOT_FOUND, "No such design.".into()),
            StepError::NoSuchEntity => ApiError(StatusCode::NOT_FOUND, "The design has no such entity.".into()),
            StepError::Stale => ApiError(StatusCode::CONFLICT, "The design was changed from somewhere else.".into()),
            StepError::Rejected(text) => ApiError(StatusCode::UNPROCESSABLE_ENTITY, text),
            StepError::Refused(e) => {
                let mut text = e.to_string();
                if let Some(first) = text.get(..1) {
                    text = first.to_uppercase() + &text[1..];
                }
                ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("{text}."))
            }
            StepError::Broken(e) => {
                tracing::error!("design could not be opened or saved: {e:#}");
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The design could not be opened or saved.".into())
            }
        }
    }
}

/// Only safe characters go in headers and file names; design names are user-supplied.
fn safe_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || " -_.".contains(c) { c } else { '_' }).collect()
}

/// The design as a `.sment`, including edits made since it was uploaded.
async fn get_design_file(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Response, ApiError> {
    let worker = state.clone();
    let worker_id = id.clone();
    let (bytes, rev) = blocking(move || Ok(worker.sessions.file(&worker.store, &worker.catalog, &worker_id)?)).await?;
    let name = state.store.summary(&id).map(|s| safe_name(&s.name)).unwrap_or_else(|| "design".into());
    Ok((
        [
            (header::CONTENT_TYPE.as_str(), "application/zip".to_string()),
            (header::CONTENT_DISPOSITION.as_str(), format!("attachment; filename=\"{name}.sment\"")),
            (header::CACHE_CONTROL.as_str(), "no-store".to_string()),
            (REV_HEADER, rev.to_string()),
        ],
        bytes,
    )
        .into_response())
}

/// Applies one editing step to a design.
async fn step(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<StepRequest>,
) -> Result<Json<StepResponse>, ApiError> {
    blocking(move || Ok(Json(state.sessions.step(&state.store, &state.catalog, &id, &request)?))).await
}

/// Moves the core of an entity to another cell, shifting the whole design round it.
async fn relocate_core(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<yard_core::api::RelocateRequest>,
) -> Result<Json<yard_core::api::Restructured>, ApiError> {
    blocking(move || {
        let [x, y, z] = request.to;
        let entity = request.entity.clone();
        let (rev, notes) = state.sessions.restructure(&state.store, &id, request.rev, |bp| {
            let to = yard_core::Pos::new(x, y, z);
            let facing = request.forward.zip(request.up);
            let mv = yard_core::relocate::Move { to, facing, dock_follows: request.dock_follows };
            yard_core::relocate::relocate(bp, &entity, mv, &state.catalog).map(|done| done.notes).map_err(|e| refusal_text(&e))
        })?;
        tracing::info!("moved the core of \"{}\" in design {id}", request.entity);
        Ok(Json(yard_core::api::Restructured { rev, entity: request.entity, notes }))
    })
    .await
}

/// Moves the rail block a docked entity hangs from, with or without the entity.
async fn move_rail(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<yard_core::api::MoveRailRequest>,
) -> Result<Json<yard_core::api::Restructured>, ApiError> {
    blocking(move || {
        let [x, y, z] = request.to;
        let (entity, child) = (request.entity.clone(), request.child.clone());
        let (rev, notes) = state.sessions.restructure(&state.store, &id, request.rev, |bp| {
            let by = yard_core::relocate::rail_shift(bp, &entity, &child, yard_core::Pos::new(x, y, z)).map_err(|e| refusal_text(&e))?;
            yard_core::relocate::move_rail(bp, &child, by, request.carried, &state.catalog).map(|done| done.notes).map_err(|e| refusal_text(&e))
        })?;
        tracing::info!("moved the rail of \"{child}\" in design {id}");
        Ok(Json(yard_core::api::Restructured { rev, entity: request.entity, notes }))
    })
    .await
}

/// Moves the cores of the sections of a dock chain, each with its dock
/// following, as an auto-fit worked out; all or none.
async fn fit_pivots(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<yard_core::api::PivotsRequest>,
) -> Result<Json<yard_core::api::Restructured>, ApiError> {
    if request.steps.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "There is no core to move.".into()));
    }
    blocking(move || {
        let steps = request.steps.clone();
        let (rev, notes) = state.sessions.restructure(&state.store, &id, request.rev, |bp| {
            let mut notes = Vec::new();
            for step in &steps {
                let [x, y, z] = step.to;
                let mv = yard_core::relocate::Move { to: yard_core::Pos::new(x, y, z), facing: None, dock_follows: true };
                let done = yard_core::relocate::relocate(bp, &step.entity, mv, &state.catalog).map_err(|e| refusal_text(&e))?;
                let who = if step.entity.is_empty() { "the main entity".to_string() } else { step.entity.clone() };
                notes.extend(done.notes.into_iter().map(|n| if steps.len() > 1 { format!("{who}: {n}") } else { n }));
            }
            Ok(notes)
        })?;
        tracing::info!("fitted the pivots of {} in design {id}", request.steps.iter().map(|s| format!("\"{}\"", s.entity)).collect::<Vec<_>>().join(", "));
        Ok(Json(yard_core::api::Restructured { rev, entity: request.steps[0].entity.clone(), notes }))
    })
    .await
}

/// Copies a docked entity, with what is docked to it, in place of another
/// or onto a new rail block.
async fn clone_docked(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<yard_core::api::CloneRequest>,
) -> Result<Json<yard_core::api::Restructured>, ApiError> {
    let place = match (&request.replace, &request.parent, request.at, request.facing) {
        (Some(entity), _, _, _) => yard_core::clone::Place::Replace { entity: entity.clone() },
        (None, Some(parent), Some([x, y, z]), Some(facing)) => yard_core::clone::Place::New { parent: parent.clone(), at: yard_core::Pos::new(x, y, z), facing },
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "Say which entity the copy replaces, or where its rail goes (parent, at, facing).".into())),
    };
    blocking(move || {
        let source = request.source.clone();
        let (rev, made) = state.sessions.restructure(&state.store, &id, request.rev, |bp| yard_core::clone::clone_docked(bp, &state.catalog, &source, &place).map_err(|e| refusal_text(&e)))?;
        tracing::info!("copied \"{source}\" as \"{}\" in design {id}", made.child);
        Ok(Json(yard_core::api::Restructured { rev, entity: made.child, notes: made.notes }))
    })
    .await
}

/// Cuts a box of blocks out of an entity and docks it back as a turret.
async fn make_turret(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<yard_core::api::TurretRequest>,
) -> Result<Json<yard_core::api::Restructured>, ApiError> {
    blocking(move || {
        let (a, b) = (request.min, request.max);
        let (min, max) = (yard_core::Pos::new(a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])), yard_core::Pos::new(a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])));
        if request.base > 5 {
            return Err(ApiError(StatusCode::BAD_REQUEST, "That is not a side of a box.".into()));
        }
        let (entity, base) = (request.entity.clone(), request.base as usize);
        let docker = request.docker.map(|d| yard_core::Pos::new(d[0], d[1], d[2]));
        let exclude: std::collections::BTreeSet<yard_core::Pos> = request.exclude.iter().map(|d| yard_core::Pos::new(d[0], d[1], d[2])).collect();
        let (rev, made) = state.sessions.restructure(&state.store, &id, request.rev, |bp| {
            yard_core::turret::make_turret_at(bp, &state.catalog, &entity, min, max, base, docker, &exclude).map_err(|e| refusal_text(&e))
        })?;
        tracing::info!("made turret {} in design {id}", made.child);
        Ok(Json(yard_core::api::Restructured { rev, entity: made.child, notes: made.notes }))
    })
    .await
}

/// Puts a design back as it was before its last relocation or turret.
async fn restore_structure(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<yard_core::api::RestoreRequest>,
) -> Result<Json<yard_core::api::Restructured>, ApiError> {
    blocking(move || {
        let rev = state.sessions.restore(&state.store, &id, request.rev)?;
        Ok(Json(yard_core::api::Restructured { rev, entity: String::new(), notes: vec!["The design is back as it was.".into()] }))
    })
    .await
}

/// What a refused change of structure says: the reason, without the library's prefix.
fn refusal_text(error: &yard_core::Error) -> String {
    match error {
        yard_core::Error::Unsupported { detail, .. } => sentence(detail),
        other => sentence(&other.to_string()),
    }
}

/// A message as a sentence: first letter up, a full stop at the end.
fn sentence(text: &str) -> String {
    let mut text = text.trim().to_string();
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    if !text.ends_with('.') {
        text.push('.');
    }
    text
}

/// Starts an empty ship: a core and nothing else.
async fn new_design(State(state): State<Arc<AppState>>, Json(request): Json<NewDesign>) -> Result<Json<Summary>, ApiError> {
    // The game's rule for blueprint names, so every design can go there.
    let name = game_name(&request.name);
    if name.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Give the design a name: letters, digits, spaces, - and _.".into()));
    }
    blocking(move || {
        // One name, one design: the game's catalog has a single blueprint per name.
        let name = free_name(&name, |n| state.store.has_name(n));
        let bp = Blueprint::new_ship(&name, &state.game_version);
        let id = Store::new_id();
        let broken = |e: String| {
            tracing::error!("creating design: {e}");
            ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The design could not be created.".into())
        };
        let bytes = bp.to_sment().map_err(|e| broken(e.to_string()))?;
        let summary = Summary::of(&id, &bp, &state.catalog, unix_now());
        let summary = state.store.save(summary, &bytes).map_err(|e| broken(format!("{e:#}")))?;
        tracing::info!("created design {id} \"{name}\"");
        Ok(Json(marked(&state, summary)))
    })
    .await
}

/// The models the assistant can run on, for the picker.
async fn chat_models(State(state): State<Arc<AppState>>) -> Json<crate::chat::Models> {
    Json(crate::chat::models(&state.chat_config).await)
}

/// Gives the assistant a message to work on. It works in the background;
/// the browser reads what it does from the log.
async fn chat_ask(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, Json(ask): Json<crate::chat::Ask>) -> Result<StatusCode, ApiError> {
    if state.store.summary(&id).is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "There is no such design.".into()));
    }
    if ask.message.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Type a message first.".into()));
    }
    let config = state.chat_config.clone();
    let chats = state.clone();
    match state.chats.ask(&config, chats, &id, ask) {
        Ok(()) => Ok(StatusCode::ACCEPTED),
        Err(refusal) => Err(ApiError(StatusCode::CONFLICT, refusal.sentence().into())),
    }
}

#[derive(Deserialize)]
struct LogParams {
    #[serde(default)]
    from: usize,
}

async fn chat_log(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, Query(params): Query<LogParams>) -> Json<crate::chat::LogPage> {
    Json(state.chats.log(&id, params.from))
}

async fn chat_stop(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> StatusCode {
    state.chats.stop(&id);
    StatusCode::NO_CONTENT
}

async fn chat_reset(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<StatusCode, ApiError> {
    if state.chats.reset(&id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(StatusCode::CONFLICT, "Stop the assistant first.".into()))
    }
}

/// The 3D model file types that can be turned into a ship.
async fn import_formats() -> Json<Vec<(String, Vec<String>)>> {
    Json(yard_import::formats().iter().map(|f| (f.name.to_string(), f.extensions.iter().map(|e| e.to_string()).collect())).collect())
}

#[derive(Deserialize)]
struct ImportParams {
    /// The uploaded file's name; its ending says what kind of file it is.
    name: String,
    #[serde(default = "default_longest")]
    longest: u32,
    #[serde(default = "yes")]
    solid: bool,
    /// How thick the shell of a hollow result is, in blocks.
    #[serde(default = "default_shell")]
    shell: u8,
    /// "y", "z", or anything else for the file format's own habit.
    #[serde(default)]
    up: String,
    #[serde(default)]
    turn: u8,
    #[serde(default)]
    mirror: bool,
    #[serde(default)]
    smooth: bool,
    /// The step height smoothing bevels, in blocks.
    #[serde(default = "default_step")]
    step: u8,
}

fn default_step() -> u8 {
    1
}

fn default_shell() -> u8 {
    1
}

fn default_longest() -> u32 {
    64
}

fn yes() -> bool {
    true
}

/// Turns an uploaded 3D model into a new ship.
async fn import_model(State(state): State<Arc<AppState>>, Query(params): Query<ImportParams>, body: Bytes) -> Result<Json<yard_core::api::ImportResult>, ApiError> {
    if body.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "The upload was empty.".into()));
    }
    blocking(move || {
        let stem = params.name.rsplit(['/', '\\']).next().unwrap_or("").rsplit_once('.').map_or(params.name.as_str(), |(stem, _)| stem);
        let wanted = Some(game_name(stem)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Imported model".into());
        let name = free_name(&wanted, |n| state.store.has_name(n));
        let choices = crate::importer::Choices {
            longest: params.longest,
            solid: params.solid,
            shell: params.shell,
            up: match params.up.as_str() {
                "y" => Some(yard_import::Up::Y),
                "z" => Some(yard_import::Up::Z),
                _ => None,
            },
            turn: params.turn,
            mirror: params.mirror,
            smooth: params.smooth,
            step: params.step.min(yard_core::smooth::MOST_STEP),
        };
        let (bp, notes) = crate::importer::ship_from_model(&state.catalog, &state.hull_palette, &state.game_version, &name, &params.name, &body, &choices)
            .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;
        let id = Store::new_id();
        let broken = |e: String| {
            tracing::error!("saving imported model: {e}");
            ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The ship could not be saved.".into())
        };
        let bytes = bp.to_sment().map_err(|e| broken(e.to_string()))?;
        let summary = Summary::of(&id, &bp, &state.catalog, unix_now());
        let summary = state.store.save(summary, &bytes).map_err(|e| broken(format!("{e:#}")))?;
        tracing::info!("imported model \"{}\" as design {id} ({} blocks)", name, summary.blocks);
        Ok(Json(yard_core::api::ImportResult { summary: marked(&state, summary), notes }))
    })
    .await
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Deserialize)]
struct UploadParams {
    /// Original file name, used to name designs whose archive does not say.
    name: Option<String>,
}

/// Accepts a blueprint as the raw request body.
async fn upload_design(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UploadParams>,
    body: Bytes,
) -> Result<Json<Summary>, ApiError> {
    if body.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "The upload was empty.".into()));
    }
    let id = Store::new_id();
    let created = unix_now();
    let file_name = params.name.unwrap_or_default();

    // Parsing a large ship takes a while; keep it off the async threads.
    let worker_state = state.clone();
    let worker_id = id.clone();
    let summary = tokio::task::spawn_blocking(move || -> Result<Summary, ApiError> {
        let mut bp = Blueprint::from_sment_with(&body, &worker_state.catalog).map_err(|e| {
            ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("Could not read \"{file_name}\" as a StarMade blueprint: {e}."))
        })?;
        // An archive with the files at its top level does not say what the
        // blueprint is called; the file's own name does.
        let mut wanted = bp.name.clone();
        if wanted == "Blueprint" {
            if let Some(stem) = file_name.rsplit('/').next().and_then(|n| n.strip_suffix(".sment")) {
                wanted = stem.to_string();
            }
        }
        // A name the game takes, and one name per design: the game's catalog
        // has a single blueprint per name.
        let wanted = Some(game_name(&wanted)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Blueprint".into());
        let unique = free_name(&wanted, |n| worker_state.store.has_name(n));
        let renamed = unique != bp.name;
        if renamed {
            bp.rename(&unique);
        }
        let summary = Summary::of(&worker_id, &bp, &worker_state.catalog, created);
        // A blueprint from an older game version is kept in the converted,
        // current form, so every later reader sees the same blocks. A renamed
        // one is rewritten so the folder inside the file carries the name.
        let stored = if bp.converted || renamed {
            bp.to_sment().map_err(|e| {
                tracing::error!("rewriting converted design {worker_id}: {e}");
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The design could not be converted.".into())
            })?
        } else {
            body.to_vec()
        };
        let summary = worker_state.store.save(summary, &stored).map_err(|e| {
            tracing::error!("saving design {worker_id}: {e:#}");
            ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The design could not be saved.".into())
        })?;
        Ok(marked(&worker_state, summary))
    })
    .await
    .map_err(|e| {
        tracing::error!("upload task failed: {e}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The upload could not be processed.".into())
    })??;

    tracing::info!("stored design {} \"{}\" ({} blocks)", summary.id, summary.name, summary.blocks);
    Ok(Json(summary))
}

/// Switches the exchange with the game server on or off for one design.
async fn switch_sync(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(switch): Json<SyncSwitch>,
) -> Result<Json<Summary>, ApiError> {
    let summary = state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
    state.sync.switch(&id, switch.on);
    Ok(Json(marked(&state, summary)))
}

/// Sends a design to the game server at its next call, without waiting for
/// the design to be left alone. Edits still on their way to disk are written
/// first. Conflict if the design is not synced or already in step.
async fn sync_now(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Json<Summary>, ApiError> {
    blocking(move || {
        state.sessions.stored_file(&state.store, &state.catalog, &id)?;
        let summary = state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
        if !state.sync.rush(&summary) {
            return Err(ApiError(StatusCode::CONFLICT, "Nothing to send: the design is not synced, or the game already has it as it is.".into()));
        }
        Ok(Json(marked(&state, summary)))
    })
    .await
}

fn no_such_version() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "No such version of that design.".into())
}

/// The versions of a design that were in step with the game, newest first.
async fn sync_history(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Json<Vec<SyncVersion>>, ApiError> {
    let summary = state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
    let mut versions = state.history.list(&id);
    versions.reverse();
    // With edits on their way to disk the design is at none of them.
    let unsaved = state.sessions.unsaved(&id);
    for v in &mut versions {
        v.current = !unsaved && v.print == summary.print;
    }
    Ok(Json(versions))
}

/// A design's kept version, if both exist.
fn kept_version(state: &AppState, id: &str, seq: u64) -> Result<(Summary, SyncVersion, Vec<u8>), ApiError> {
    let summary = state.store.summary(id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
    let version = state.history.version(id, seq).ok_or_else(no_such_version)?;
    let bytes = state.history.file(id, seq).ok_or_else(no_such_version)?;
    Ok((summary, version, bytes))
}

/// One kept version as a `.sment`.
async fn version_file(State(state): State<Arc<AppState>>, UrlPath((id, seq)): UrlPath<(String, u64)>) -> Result<Response, ApiError> {
    let (_, version, bytes) = blocking(move || kept_version(&state, &id, seq)).await?;
    let name = safe_name(&version.name);
    Ok((
        [
            (header::CONTENT_TYPE.as_str(), "application/zip".to_string()),
            (header::CONTENT_DISPOSITION.as_str(), format!("attachment; filename=\"{name} v{seq}.sment\"")),
            // A version never changes, but its number is the design's own business.
            (header::CACHE_CONTROL.as_str(), "no-store".to_string()),
        ],
        bytes,
    )
        .into_response())
}

fn unreadable_version(id: &str, seq: u64, e: impl std::fmt::Display) -> ApiError {
    tracing::error!("version {seq} of design {id}: {e}");
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "That version could not be read.".into())
}

/// Makes a kept version the design's contents again. The design keeps the
/// name it has now, which is what pairs it with its blueprint in the game;
/// being different from what the game has, it goes there like any other edit.
/// What the design held until now is not lost: if it is not one of the kept
/// versions already (edits made here that the game never saw), it becomes one.
async fn restore_version(State(state): State<Arc<AppState>>, UrlPath((id, seq)): UrlPath<(String, u64)>) -> Result<Json<Summary>, ApiError> {
    blocking(move || {
        let (_, _, bytes) = kept_version(&state, &id, seq)?;
        let mut bp = Blueprint::from_sment(&bytes).map_err(|e| unreadable_version(&id, seq, e))?;
        let summary = state
            .sessions
            .replacing(&state.store, &state.catalog, &id, || {
                // Any edits not yet on disk are there by now.
                let current = state.store.summary(&id).context("the design is gone")?;
                if !state.history.list(&id).iter().any(|v| v.print == current.print) {
                    // Without a copy of the edits, they are not replaced.
                    state
                        .history
                        .record(&state.store, &current, yard_core::api::SyncDirection::Unsynced, unix_now(), None)?
                        .context("the design's unsynced edits could not be kept")?;
                }
                let stored = if bp.name == current.name {
                    bytes
                } else {
                    bp.rename(&current.name);
                    bp.to_sment()?
                };
                state.store.replace(Summary::of(&id, &bp, &state.catalog, current.created), &stored)
            })
            .map_err(|e| {
                tracing::error!("putting design {id} back to version {seq}: {e:#}");
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, "That version could not be put back. The design is as it was.".into())
            })?;
        tracing::info!("design {id} \"{}\" was put back to version {seq} of its sync history", summary.name);
        Ok(Json(marked(&state, summary)))
    })
    .await
}

/// Stores a kept version as a design of its own. It starts out of the
/// exchange with the game, so that looking at an old version does not put a
/// new blueprint into the game's catalog.
async fn copy_version(State(state): State<Arc<AppState>>, UrlPath((id, seq)): UrlPath<(String, u64)>) -> Result<Json<Summary>, ApiError> {
    blocking(move || {
        let (_, version, bytes) = kept_version(&state, &id, seq)?;
        let mut bp = Blueprint::from_sment(&bytes).map_err(|e| unreadable_version(&id, seq, e))?;
        // "<name> v<seq>", with the name cut short if the two do not fit.
        let suffix = format!(" v{seq}");
        let room = MAX_NAME.saturating_sub(suffix.len());
        let base: String = game_name(&version.name).chars().take(room).collect();
        let name = free_name(&format!("{}{suffix}", base.trim_end()), |n| state.store.has_name(n));
        bp.rename(&name);
        let stored = bp.to_sment().map_err(|e| unreadable_version(&id, seq, e))?;
        let new_id = Store::new_id();
        // Off before it exists, so no report from the game can pick it up in between.
        state.sync.switch(&new_id, false);
        let summary = state
            .store
            .save(Summary::of(&new_id, &bp, &state.catalog, unix_now()), &stored)
            .map_err(|e| unreadable_version(&id, seq, format!("{e:#}")))?;
        tracing::info!("version {seq} of design {id} was stored as design {new_id} \"{name}\"");
        Ok(Json(marked(&state, summary)))
    })
    .await
}

async fn sync_info(State(state): State<Arc<AppState>>) -> Json<SyncInfo> {
    Json(state.sync.info())
}

impl From<SyncError> for ApiError {
    fn from(e: SyncError) -> Self {
        match e {
            SyncError::Unreadable(why) => {
                ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("That is not a StarMade blueprint this server can read: {why}."))
            }
            SyncError::Mismatch => {
                ApiError(StatusCode::CONFLICT, "The blueprint changed while it was being sent. Send it again.".into())
            }
            SyncError::NoSuchDesign => ApiError(StatusCode::NOT_FOUND, "No such design.".into()),
            SyncError::SwitchedOff => ApiError(StatusCode::CONFLICT, "Syncing is switched off for that design.".into()),
            SyncError::Broken(e) => {
                tracing::error!("sync: {e:#}");
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, "The design could not be stored.".into())
            }
        }
    }
}

/// Requests from a game server's agent carry the shared secret, if one is set.
fn agent_allowed(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(wanted) = &state.sync_token else { return Ok(()) };
    let given = headers.get(TOKEN_HEADER).and_then(|v| v.to_str().ok()).unwrap_or("");
    // Compared in full whatever the input, so timing says nothing about the secret.
    let same = wanted.len() == given.len() && wanted.bytes().zip(given.bytes()).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0;
    same.then_some(()).ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "This server wants its sync secret.".into()))
}

/// A game server's agent says what the game has and is told what to move.
async fn sync_report(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(report): Json<GameReport>,
) -> Result<Json<SyncPlan>, ApiError> {
    agent_allowed(&state, &headers)?;
    blocking(move || Ok(Json(state.sync.report(&state, &report)))).await
}

#[derive(Deserialize)]
struct ReceiveParams {
    /// The blueprint's name in the game.
    name: String,
    /// The fingerprint the agent announced it with.
    print: String,
}

/// A blueprint from the game, as a `.sment` in the request body.
async fn sync_receive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ReceiveParams>,
    body: Bytes,
) -> Result<Json<Summary>, ApiError> {
    agent_allowed(&state, &headers)?;
    let name = params.name;
    if !valid_name(&name) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "The game itself would not take that blueprint name.".into()));
    }
    blocking(move || Ok(Json(state.sync.receive(&state, &name, &params.print, &body)?))).await
}

/// A design for the game to import, with its fingerprint in a header.
async fn sync_file(State(state): State<Arc<AppState>>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Result<Response, ApiError> {
    agent_allowed(&state, &headers)?;
    let (bytes, print) = blocking(move || Ok(state.sync.file(&state, &id)?)).await?;
    Ok(([(header::CONTENT_TYPE.as_str(), "application/zip".to_string()), (PRINT_HEADER, print)], bytes).into_response())
}

/// The agent confirms the game has a design.
async fn sync_done(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(done): Json<SyncDone>) -> Result<StatusCode, ApiError> {
    agent_allowed(&state, &headers)?;
    blocking(move || Ok(state.sync.done(&state, &done)?)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The shipyards the game server last reported, to link designs to.
async fn sync_yards(State(state): State<Arc<AppState>>) -> Json<Vec<GameYard>> {
    Json(state.sync.yards())
}

/// Where a design stands with the shipyard design it is linked to; null if none.
async fn yard_link(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Json<Option<YardLinkInfo>>, ApiError> {
    blocking(move || {
        state.store.summary(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such design.".into()))?;
        Ok(Json(state.sync.yard_link(&state.store, &id)))
    })
    .await
}

/// Links a design to a design in a shipyard in the game, or to a new one.
async fn link_yard(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<YardLinkRequest>,
) -> Result<Json<Option<YardLinkInfo>>, ApiError> {
    blocking(move || {
        match state.sync.link_yard(&state.store, &id, &request) {
            Ok(()) => {}
            Err(LinkError::NoSuchDesign) => return Err(ApiError(StatusCode::NOT_FOUND, "No such design.".into())),
            Err(LinkError::Refused(why)) => return Err(ApiError(StatusCode::CONFLICT, why)),
        }
        Ok(Json(state.sync.yard_link(&state.store, &id)))
    })
    .await
}

async fn unlink_yard(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> StatusCode {
    if state.sync.unlink_yard(&id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[derive(Deserialize)]
struct YardTakeParams {
    design: String,
    /// The shipyard design's unique name.
    uid: String,
    print: String,
}

/// A copy of a shipyard design from the game, as a zipped blueprint.
async fn yard_take(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<YardTakeParams>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    agent_allowed(&state, &headers)?;
    blocking(move || Ok(state.sync.take_yard(&state, &params.design, &params.uid, &params.print, &body)?)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The agent confirms a shipyard holds a design made from a design's blueprint.
async fn yard_given(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(given): Json<YardGiven>) -> Result<StatusCode, ApiError> {
    agent_allowed(&state, &headers)?;
    blocking(move || Ok(state.sync.yard_given(&state, &given)?)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The agent says what went wrong with a shipyard design.
async fn yard_trouble(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(trouble): Json<YardTrouble>) -> Result<StatusCode, ApiError> {
    agent_allowed(&state, &headers)?;
    state.sync.yard_trouble(&trouble.design, &trouble.why);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use yard_core::edit::{FillMode, Op, When};
    use yard_core::{Block, Pos};

    /// A StarMade install to read block definitions from, or None to skip.
    fn starmade_dir() -> Option<PathBuf> {
        let dir = std::env::var_os("STARMADE_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".smtest2/vol")))?;
        dir.join("data/config/BlockConfig.xml").exists().then_some(dir)
    }

    /// Removes a test's data directory when the test ends, pass or fail.
    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn test_app(name: &str) -> Option<(Router, Arc<AppState>, Cleanup)> {
        let Some(starmade_dir) = starmade_dir() else {
            eprintln!("skipped: no StarMade install (set STARMADE_DIR)");
            return None;
        };
        let data_dir = std::env::temp_dir().join(format!("yard-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        let cleanup = Cleanup(data_dir.clone());
        // Tests do not wait for a design to settle before it may go to the game.
        let config = Config { starmade_dir, data_dir, static_dir: PathBuf::from("missing"), sync_token: token(name), sync_quiet_secs: 0 };
        let state = AppState::load(config).await.unwrap();
        Some((router(state.clone()), state, cleanup))
    }

    /// The sync secret for a test: only the test about the secret has one.
    fn token(test: &str) -> Option<String> {
        (test == "secret").then(|| "s3cret".to_string())
    }

    async fn send(app: &Router, method: &str, uri: &str, json: Option<String>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut request = Request::builder().method(method).uri(uri);
        if json.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let response = app.clone().oneshot(request.body(Body::from(json.unwrap_or_default())).unwrap()).await.unwrap();
        let (parts, body) = response.into_parts();
        (parts.status, parts.headers, body.collect().await.unwrap().to_bytes().to_vec())
    }

    async fn download(app: &Router, id: &str) -> (Blueprint, u64) {
        let (status, headers, body) = send(app, "GET", &format!("/api/designs/{id}/file"), None).await;
        assert_eq!(status, StatusCode::OK);
        let rev = headers.get(REV_HEADER).unwrap().to_str().unwrap().parse().unwrap();
        (Blueprint::from_sment(&body).unwrap(), rev)
    }

    async fn step(app: &Router, id: &str, rev: u64, ops: Vec<Op>) -> (StatusCode, Vec<u8>) {
        let body = serde_json::to_string(&StepRequest { rev, entity: String::new(), ops }).unwrap();
        let (status, _, bytes) = send(app, "POST", &format!("/api/designs/{id}/steps"), Some(body)).await;
        (status, bytes)
    }

    fn hull_row(from: i32, to: i32) -> Op {
        Op::Fill {
            min: Pos::new(from, 0, 0),
            max: Pos::new(to, 0, 0),
            block: Block::new(5, 0, false),
            mode: FillMode::Solid,
            when: When::Empty,
            link: None,
        }
    }

    #[tokio::test]
    async fn a_page_left_open_while_the_design_was_idle_stays_in_step() {
        let Some((app, state, _cleanup)) = test_app("idle").await else { return };
        let (status, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Idle Ship"}"#.into())).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let summary: Summary = serde_json::from_slice(&body).unwrap();
        let (_, rev) = download(&app, &summary.id).await;
        let (status, body) = step(&app, &summary.id, rev, vec![hull_row(0, 3)]).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let done: StepResponse = serde_json::from_slice(&body).unwrap();

        // Saved, then dropped from memory for being idle.
        state.sessions.flush(&state.store, &state.catalog, true);
        state.sessions.flush_forgetting_after(&state.store, &state.catalog, false, std::time::Duration::ZERO);

        // The page that had it open goes on where it was.
        let (status, body) = step(&app, &summary.id, done.rev, vec![hull_row(5, 6)]).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let (bp, _) = download(&app, &summary.id).await;
        assert_eq!(bp.root.entity.block_count(), 6);
    }

    #[tokio::test]
    async fn a_new_design_can_be_edited_undone_and_downloaded() {
        let Some((app, state, _cleanup)) = test_app("edit").await else { return };
        let (status, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Test Ship"}"#.into())).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let summary: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!((summary.name.as_str(), summary.blocks), ("Test Ship", 1));

        let (bp, rev) = download(&app, &summary.id).await;
        assert_eq!(bp.root.entity.get(Pos::new(0, 0, 0)).ty(), Catalog::CORE);

        // Three hull blocks to the right of the core.
        let (status, body) = step(&app, &summary.id, rev, vec![hull_row(0, 3)]).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let done: StepResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!((done.rev, done.undo, done.redo), (rev + 1, 1, 0));
        assert!(done.change.is_none());

        // A second editor that has not seen that step is told to reload.
        let (status, _) = step(&app, &summary.id, rev, vec![hull_row(5, 6)]).await;
        assert_eq!(status, StatusCode::CONFLICT);

        // An edit that is too big is refused and changes nothing.
        let huge = Op::Fill {
            min: Pos::new(0, 0, 0),
            max: Pos::new(300, 300, 300),
            block: Block::new(5, 0, false),
            mode: FillMode::Solid,
            when: When::Empty,
            link: None,
        };
        let (status, body) = step(&app, &summary.id, done.rev, vec![huge]).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8_lossy(&body).contains("most allowed"));

        // The download includes the edit, with a header that matches the blocks.
        let (bp, rev_now) = download(&app, &summary.id).await;
        assert_eq!(rev_now, done.rev);
        assert_eq!(bp.root.entity.block_count(), 4);
        assert_eq!(bp.root.header.counts, vec![(1, 1), (5, 3)]);
        assert_eq!(state.store.summary(&summary.id).unwrap().blocks, 4);

        // Undo reports what it put back so the editor can follow.
        let (status, body) = step(&app, &summary.id, rev_now, vec![Op::Undo]).await;
        assert_eq!(status, StatusCode::OK);
        let undone: StepResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(undone.change.as_ref().unwrap().cells.len(), 3);
        assert_eq!((undone.undo, undone.redo), (0, 1));
        let (bp, _) = download(&app, &summary.id).await;
        assert_eq!(bp.root.entity.block_count(), 1);

        let (status, body) = step(&app, &summary.id, undone.rev, vec![Op::Undo]).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8_lossy(&body).contains("nothing to undo"), "{}", String::from_utf8_lossy(&body));
    }

    #[tokio::test]
    async fn edits_survive_being_flushed_and_reopened() {
        let Some((app, state, _cleanup)) = test_app("flush").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Keeper"}"#.into())).await;
        let summary: Summary = serde_json::from_slice(&body).unwrap();
        let (_, rev) = download(&app, &summary.id).await;
        let (status, _) = step(&app, &summary.id, rev, vec![hull_row(1, 2)]).await;
        assert_eq!(status, StatusCode::OK);
        state.flush(true);
        // What is on disk now is a blueprint with the two new blocks.
        let on_disk = Blueprint::from_sment(&state.store.file(&summary.id).unwrap()).unwrap();
        assert_eq!(on_disk.root.entity.block_count(), 3);
        assert_eq!(state.store.list()[0].blocks, 3);
    }

    #[tokio::test]
    async fn a_3d_model_becomes_a_ship_of_hull_blocks() {
        let Some((app, state, _cleanup)) = test_app("import").await else { return };
        // A unit cube with the near face red and the rest grey, as an OBJ.
        let obj = "mtllib c.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
                   usemtl red\nf 5 6 7 8\nusemtl grey\nf 1 4 3 2\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n";
        let mtl = "newmtl red\nKd 0.8 0.05 0.05\nnewmtl grey\nKd 0.45 0.45 0.45\n";
        let _ = mtl; // companion files only travel in a zip; the cube is read without colours.
        let request = |uri: &str, body: Vec<u8>| Request::builder().method("POST").uri(uri.to_string()).body(Body::from(body)).unwrap();
        let response = app.clone().oneshot(request("/api/designs/import?name=Cube.obj&longest=9&solid=true", obj.as_bytes().to_vec())).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let result: yard_core::api::ImportResult = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.summary.name, "Cube");
        assert_eq!(result.summary.blocks, 9 * 9 * 9);
        assert!(result.notes.iter().any(|n| n.contains("729")), "{:?}", result.notes);
        // The design is a real one: it downloads and reads back with a core.
        let (bp, _) = download(&app, &result.summary.id).await;
        assert_eq!(bp.root.entity.get(yard_core::Pos::new(0, 0, 0)).ty(), 1);
        // A hollow one has no inside.
        let response = app.clone().oneshot(request("/api/designs/import?name=Cube.obj&longest=9&solid=false", obj.as_bytes().to_vec())).await.unwrap();
        let result: yard_core::api::ImportResult = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(result.summary.name, "Cube 2");
        assert_eq!(result.summary.blocks, 9 * 9 * 9 - 7 * 7 * 7);
        // And its shell can be thicker.
        let response = app.clone().oneshot(request("/api/designs/import?name=Cube.obj&longest=9&solid=false&shell=2", obj.as_bytes().to_vec())).await.unwrap();
        let result: yard_core::api::ImportResult = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(result.summary.blocks, 9 * 9 * 9 - 5 * 5 * 5);
        // Files that are not models, and models with nothing in them, are refused with a sentence.
        for (name, body) in [("x.obj", b"hello".to_vec()), ("x.blend", vec![0; 64]), ("x.stl", Vec::new())] {
            let response = app.clone().oneshot(request(&format!("/api/designs/import?name={name}"), body)).await.unwrap();
            assert!(response.status() == StatusCode::UNPROCESSABLE_ENTITY || response.status() == StatusCode::BAD_REQUEST, "{name}: {}", response.status());
        }
        assert!(!state.hull_palette.is_empty());
        let (status, _, body) = send(&app, "GET", "/api/import/formats", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8_lossy(&body).contains("Wavefront OBJ"));
    }

    #[tokio::test]
    async fn the_core_moves_a_turret_is_cut_out_and_either_can_be_put_back() {
        let Some((app, _state, _cleanup)) = test_app("structure").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Frame"}"#.into())).await;
        let id = serde_json::from_slice::<Summary>(&body).unwrap().id;
        let (_, rev) = download(&app, &id).await;
        // A slab of hull 7 by 2 by 7 under the core, and a block above it.
        let hull = Op::Fill { min: Pos::new(-3, -2, -3), max: Pos::new(3, -1, 3), block: Block::new(5, 0, false), mode: FillMode::Solid, when: When::Empty, link: None };
        let (status, body) = step(&app, &id, rev, vec![hull]).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let (before, rev) = download(&app, &id).await;
        let post = |uri: String, body: String| {
            let app = app.clone();
            async move { send(&app, "POST", &uri, Some(body)).await }
        };
        // The core goes down into the slab: everything shifts by one.
        let (status, _, body) = post(format!("/api/designs/{id}/relocate-core"), format!(r#"{{"rev":{rev},"entity":"","to":[0,-1,0]}}"#)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let done: yard_core::api::Restructured = serde_json::from_slice(&body).unwrap();
        assert!(done.notes.iter().any(|n| n.contains("(0, -1, 0)")), "{:?}", done.notes);
        let (moved, rev) = download(&app, &id).await;
        assert_eq!(moved.root.entity.block_count(), before.root.entity.block_count());
        assert_eq!(moved.root.entity.get(Pos::new(0, 1, 0)).ty(), 5, "the block that was in the core's new cell took its old one");
        // A stale copy is refused; so is a cell that is the core itself.
        let (status, _, _) = post(format!("/api/designs/{id}/relocate-core"), format!(r#"{{"rev":{},"entity":"","to":[1,0,0]}}"#, rev - 1)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _, body) = post(format!("/api/designs/{id}/relocate-core"), format!(r#"{{"rev":{rev},"entity":"","to":[0,0,0]}}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!String::from_utf8_lossy(&body).contains("nsupported"), "{}", String::from_utf8_lossy(&body));
        // Put back: the design is as it was before the move.
        let (status, _, body) = post(format!("/api/designs/{id}/restore"), format!(r#"{{"rev":{rev}}}"#)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let (back, rev) = download(&app, &id).await;
        assert_eq!(back.root.entity.get(Pos::new(0, 0, 0)).ty(), Catalog::CORE);
        assert_eq!(back.root.entity.get(Pos::new(0, -1, 0)).ty(), 5);
        // Only once.
        let (status, _, _) = post(format!("/api/designs/{id}/restore"), format!(r#"{{"rev":{rev}}}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        // A turret: a 3 by 3 by 3 box on top of the slab, beside the core.
        let block = Op::Fill { min: Pos::new(1, 0, 1), max: Pos::new(3, 2, 3), block: Block::new(5, 0, false), mode: FillMode::Solid, when: When::Empty, link: None };
        let (status, _) = step(&app, &id, rev, vec![block]).await;
        assert_eq!(status, StatusCode::OK);
        let (_, rev) = download(&app, &id).await;
        let (status, _, body) = post(format!("/api/designs/{id}/make-turret"), format!(r#"{{"rev":{rev},"entity":"","min":[3,2,3],"max":[1,0,1],"base":3}}"#)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let made: yard_core::api::Restructured = serde_json::from_slice(&body).unwrap();
        assert_eq!(made.entity, "ATTACHED_0");
        let (with_turret, rev) = download(&app, &id).await;
        assert_eq!(with_turret.root.children.len(), 1);
        let child = with_turret.entity("ATTACHED_0").unwrap();
        assert_eq!(child.entity.get(Pos::new(0, 0, 0)).ty(), Catalog::CORE, "the turret has its own core");
        assert_eq!(with_turret.root.entity.get(Pos::new(2, -1, 2)).ty(), yard_core::dock::TURRET_AXIS, "on an axis under the middle of the box");
        // Any edit and there is nothing to put back.
        let (status, _) = step(&app, &id, rev, vec![hull_row(5, 6)]).await;
        assert_eq!(status, StatusCode::OK);
        let (_, rev) = download(&app, &id).await;
        let (status, _, _) = post(format!("/api/designs/{id}/restore"), format!(r#"{{"rev":{rev}}}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn a_docked_entity_is_copied_onto_a_new_rail_and_in_place_of_another() {
        let Some((app, _state, _cleanup)) = test_app("clone").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Twins"}"#.into())).await;
        let id = serde_json::from_slice::<Summary>(&body).unwrap().id;
        let (_, rev) = download(&app, &id).await;
        let hull = Op::Fill { min: Pos::new(-6, -2, -6), max: Pos::new(6, -1, 6), block: Block::new(5, 0, false), mode: FillMode::Solid, when: When::Empty, link: None };
        let block = Op::Fill { min: Pos::new(1, 0, 1), max: Pos::new(3, 2, 3), block: Block::new(5, 0, false), mode: FillMode::Solid, when: When::Empty, link: None };
        let (status, _) = step(&app, &id, rev, vec![hull, block]).await;
        assert_eq!(status, StatusCode::OK);
        let (_, rev) = download(&app, &id).await;
        let post = |uri: String, body: String| {
            let app = app.clone();
            async move { send(&app, "POST", &uri, Some(body)).await }
        };
        let (status, _, body) = post(format!("/api/designs/{id}/make-turret"), format!(r#"{{"rev":{rev},"entity":"","min":[3,2,3],"max":[1,0,1],"base":3}}"#)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let (_, rev) = download(&app, &id).await;
        // Onto a new axis on the other side of the slab.
        let (status, _, body) = post(format!("/api/designs/{id}/clone-docked"), format!(r#"{{"rev":{rev},"source":"ATTACHED_0","parent":"","at":[-2,-1,-2],"facing":[0,1,0]}}"#)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let done: yard_core::api::Restructured = serde_json::from_slice(&body).unwrap();
        assert_eq!(done.entity, "ATTACHED_1");
        assert!(done.notes[0].starts_with("Copied the docked entity ATTACHED_0 as ATTACHED_1"), "{:?}", done.notes);
        let (bp, rev) = download(&app, &id).await;
        assert_eq!(bp.root.children.len(), 2);
        assert_eq!(bp.root.entity.get(Pos::new(-2, -1, -2)).ty(), yard_core::dock::TURRET_AXIS);
        let docks = bp.docks();
        assert_eq!(docks["ATTACHED_1"].rail.pos, Pos::new(-2, -1, -2));
        assert_eq!(bp.entity("ATTACHED_1").unwrap().entity.block_count(), bp.entity("ATTACHED_0").unwrap().entity.block_count());
        // In place of the original, which goes.
        let (status, _, body) = post(format!("/api/designs/{id}/clone-docked"), format!(r#"{{"rev":{rev},"source":"ATTACHED_1","replace":"ATTACHED_0"}}"#)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let done: yard_core::api::Restructured = serde_json::from_slice(&body).unwrap();
        assert_eq!(done.entity, "ATTACHED_0");
        let (bp, rev) = download(&app, &id).await;
        assert_eq!(bp.root.children.len(), 2);
        assert_eq!(bp.docks()["ATTACHED_0"].rail.pos, Pos::new(2, -1, 2), "the copy hangs from the old turret's axis");
        // Half a request, and a copy in its own place, are refused with a sentence.
        let (status, _, _) = post(format!("/api/designs/{id}/clone-docked"), format!(r#"{{"rev":{rev},"source":"ATTACHED_0"}}"#)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _, body) = post(format!("/api/designs/{id}/clone-docked"), format!(r#"{{"rev":{rev},"source":"ATTACHED_0","replace":"ATTACHED_0"}}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8_lossy(&body).contains("its own place"), "{}", String::from_utf8_lossy(&body));
        // Put back undoes the last copy.
        let (status, _, _) = post(format!("/api/designs/{id}/restore"), format!(r#"{{"rev":{rev}}}"#)).await;
        assert_eq!(status, StatusCode::OK);
        let (bp, _) = download(&app, &id).await;
        assert_eq!(bp.root.children.len(), 2);
        assert_eq!(bp.docks()["ATTACHED_1"].rail.pos, Pos::new(-2, -1, -2));
    }

    #[tokio::test]
    async fn a_design_can_be_deleted_and_its_files_are_set_aside() {
        let Some((app, state, _cleanup)) = test_app("delete").await else { return };
        let (status, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Doomed"}"#.into())).await;
        assert_eq!(status, StatusCode::OK);
        let summary: Summary = serde_json::from_slice(&body).unwrap();
        // It is open (and has an edit not yet on disk) when it is deleted.
        let (_, rev) = download(&app, &summary.id).await;
        let (status, _) = step(&app, &summary.id, rev, vec![hull_row(0, 3)]).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = send(&app, "DELETE", &format!("/api/designs/{}", summary.id), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Gone from the list and from everything that asks for it.
        let (_, _, listed) = send(&app, "GET", "/api/designs", None).await;
        assert!(!String::from_utf8_lossy(&listed).contains(&summary.id));
        for uri in [format!("/api/designs/{}", summary.id), format!("/api/designs/{}/file", summary.id)] {
            let (status, _, _) = send(&app, "GET", &uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        }
        let (status, _, _) = send(&app, "DELETE", &format!("/api/designs/{}", summary.id), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // The name is free again, and the files are in the trash, edit included.
        assert!(!state.store.has_name("Doomed"));
        let kept: Vec<_> = std::fs::read_dir(state.store.trash_dir()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(kept.iter().any(|n| n.starts_with(&summary.id) && n.ends_with(".sment")), "{kept:?}");
        let sment = std::fs::read(state.store.trash_dir().join(kept.iter().find(|n| n.ends_with(".sment")).unwrap())).unwrap();
        assert_eq!(Blueprint::from_sment(&sment).unwrap().root.entity.block_count(), 4, "the edit not yet saved is in the kept file");
    }

    #[tokio::test]
    async fn smoothing_gives_a_slope_wedges_and_never_slabs() {
        let Some((app, _state, _cleanup)) = test_app("smooth").await else { return };
        // A ramp 12 long, 6 wide and 12 high at the back: a slope of 45 degrees.
        let points = [[0.0f32, 0.0, 0.0], [12.0, 0.0, 0.0], [12.0, 12.0, 0.0], [0.0, 0.0, 6.0], [12.0, 0.0, 6.0], [12.0, 12.0, 6.0]];
        let faces: [&[usize]; 5] = [&[0, 1, 2], &[3, 5, 4], &[0, 3, 4, 1], &[1, 4, 5, 2], &[0, 2, 5, 3]];
        let middle = [0, 1, 2].map(|k| points.iter().map(|p| p[k]).sum::<f32>() / 6.0);
        let mut obj: String = points.iter().map(|p| format!("v {} {} {}\n", p[0], p[1], p[2])).collect();
        for face in faces {
            for i in 1..face.len() - 1 {
                let mut corner = [face[0], face[i], face[i + 1]];
                let d = |a: usize, b: usize| [0, 1, 2].map(|k| points[b][k] - points[a][k]);
                let (a, b) = (d(corner[0], corner[1]), d(corner[0], corner[2]));
                let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
                if (0..3).map(|k| n[k] * (points[corner[0]][k] - middle[k])).sum::<f32>() < 0.0 {
                    corner.swap(1, 2);
                }
                obj += &format!("f {} {} {}\n", corner[0] + 1, corner[1] + 1, corner[2] + 1);
            }
        }
        let request = |uri: &str| Request::builder().method("POST").uri(uri.to_string()).body(Body::from(obj.clone().into_bytes())).unwrap();
        let import = |uri: &'static str| {
            let app = app.clone();
            let request = request(uri);
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                serde_json::from_slice::<yard_core::api::ImportResult>(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
            }
        };
        let plain = import("/api/designs/import?name=Ramp.obj&longest=12&solid=true&up=y").await;
        assert!(plain.summary.top_types.iter().all(|t| t.name == "Grey Basic Armor" || t.name == "Ship Core"), "{:?}", plain.summary.top_types);
        assert!(!plain.notes.iter().any(|n| n.contains("Smoothed")));
        let smooth = import("/api/designs/import?name=Ramp.obj&longest=12&solid=true&up=y&smooth=true").await;
        let names: Vec<&str> = smooth.summary.top_types.iter().map(|t| t.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("Wedge")), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("1/4") || n.contains("1/2") || n.contains("3/4")), "slabs in {names:?}");
        assert!(smooth.notes.iter().any(|n| n.contains("Smoothed")), "{:?}", smooth.notes);
        // The slope's blocks are still in the right place: the design reads back.
        let (bp, _) = download(&app, &smooth.summary.id).await;
        assert_eq!(bp.root.entity.get(yard_core::Pos::new(0, 0, 0)).ty(), 1);
    }

    #[tokio::test]
    async fn the_assistant_builds_in_an_open_design_and_the_browser_can_follow() {
        use yard_agent::{Script, ScriptedProvider};
        let Some((app, state, _cleanup)) = test_app("assistant").await else { return };
        let (status, _, body) = send(&app, "POST", "/api/designs/new", Some(serde_json::to_string(&NewDesign { name: "Chat ship".into() }).unwrap())).await;
        assert_eq!(status, StatusCode::OK);
        let id = serde_json::from_slice::<Summary>(&body).unwrap().id;
        // Without keys or a local server the picker says why it is short.
        let (status, _, body) = send(&app, "GET", "/api/chat/models", None).await;
        assert_eq!(status, StatusCode::OK);
        let offered: crate::chat::Models = serde_json::from_slice(&body).unwrap();
        assert!(!offered.notes.is_empty() || !offered.models.is_empty());
        // Models that cannot be used are refused with a sentence.
        let ask = |provider: &str| serde_json::json!({"provider": provider, "model": "m", "message": "build a hull"}).to_string();
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{id}/chat"), Some(ask("nonsense"))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(String::from_utf8_lossy(&body).contains("Choose a model"));
        // A scripted model fills a box, then answers.
        let script = vec![
            Script::calls_saying("Building.", &[("fill", serde_json::json!({"from": [-2, -1, -3], "to": [2, 1, 6], "block": "Grey Basic Armor"}))]),
            Script::says("Done: a grey hull."),
        ];
        state.chats.start(Box::new(ScriptedProvider::new(script)), state.clone(), &id, "build a hull".to_string(), String::new()).ok().unwrap();
        let mut page = crate::chat::LogPage::default();
        for _ in 0..100 {
            page = state.chats.log(&id, 0);
            if !page.running && page.entries.len() > 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(!page.running);
        let kinds: Vec<&str> = page.entries.iter().map(|e| match e { crate::chat::Entry::You { .. } => "you", crate::chat::Entry::Agent { .. } => "agent", crate::chat::Entry::Step { .. } => "step" }).collect();
        assert_eq!(kinds.first(), Some(&"you"));
        assert!(kinds.contains(&"step"), "{kinds:?}");
        assert!(page.entries.iter().any(|e| matches!(e, crate::chat::Entry::Agent { event: yard_agent::Event::Done { .. } })));
        // The design really changed, and a browser that was in step can follow.
        let step = page.entries.iter().find_map(|e| if let crate::chat::Entry::Step { before, after, change, .. } = e { Some((*before, *after, change.cells.len())) } else { None }).unwrap();
        assert!(step.1 > step.0 && step.2 > 0);
        let (bp, rev) = download(&app, &id).await;
        assert_eq!(rev, step.1);
        assert_eq!(bp.root.entity.get(yard_core::Pos::new(1, 0, 2)).ty(), 598);
        // Asking from where it left off gives nothing new; a second message works; reset forgets.
        assert!(state.chats.log(&id, page.next).entries.is_empty());
        state.chats.start(Box::new(ScriptedProvider::new(vec![Script::says("Hello again.")])), state.clone(), &id, "hi".to_string(), String::new()).ok().unwrap();
        for _ in 0..100 {
            if !state.chats.log(&id, page.next).running {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(state.chats.log(&id, page.next).entries.len() >= 2);
        assert!(state.chats.reset(&id));
        assert!(state.chats.log(&id, 0).entries.is_empty());
    }

    #[tokio::test]
    async fn bad_names_and_unknown_designs_are_refused() {
        let Some((app, _, _cleanup)) = test_app("refuse").await else { return };
        for name in ["", "   ", "../../etc"] {
            let body = serde_json::to_string(&NewDesign { name: name.into() }).unwrap();
            let (status, _, _) = send(&app, "POST", "/api/designs/new", Some(body)).await;
            // Path characters are replaced, never passed through.
            if status == StatusCode::OK {
                assert_eq!(name, "../../etc");
            } else {
                assert_eq!(status, StatusCode::BAD_REQUEST);
            }
        }
        let (status, _) = step(&app, "aaaaaaaaaaaa", 0, vec![hull_row(1, 2)]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _, _) = send(&app, "GET", "/api/designs/..%2F..%2Fx/file", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// A ship as the game would hold it: a core and `hull` blocks in a row.
    fn game_ship(name: &str, hull: i32) -> (Vec<u8>, String) {
        let mut bp = Blueprint::new_ship(name, "0.304.7_test");
        for x in 1..=hull {
            bp.root.entity.set(Pos::new(x, 0, 0), Block::new(5, 0, false));
        }
        bp.root.header.refresh(&bp.root.entity);
        let sment = bp.to_sment().unwrap();
        let print = crate::sync::print_of_sment(&sment).unwrap();
        (sment, print)
    }

    async fn report(app: &Router, game: &[(&str, &str)]) -> SyncPlan {
        let blueprints = game.iter().map(|(name, print)| serde_json::json!({ "name": name, "print": print })).collect::<Vec<_>>();
        let body = serde_json::json!({ "server": "test game", "blueprints": blueprints }).to_string();
        let (status, _, bytes) = send(app, "POST", "/api/sync/report", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn put_blueprint(app: &Router, name: &str, print: &str, sment: Vec<u8>) -> (StatusCode, Vec<u8>) {
        let uri = format!("/api/sync/blueprints?name={}&print={print}", name.replace(' ', "%20"));
        let request = Request::builder().method("PUT").uri(uri).body(Body::from(sment)).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        (status, response.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    /// The agent's part of handing a design to the game: download it, then
    /// confirm with the fingerprint the game's copy has after the import.
    async fn import_into_game(app: &Router, design: &str, name: &str) -> (Blueprint, String) {
        let (status, headers, bytes) = send(app, "GET", &format!("/api/sync/designs/{design}/file"), None).await;
        assert_eq!(status, StatusCode::OK);
        let print = headers.get(PRINT_HEADER).unwrap().to_str().unwrap().to_string();
        assert_eq!(crate::sync::print_of_sment(&bytes).unwrap(), print);
        let done = serde_json::json!({ "design": design, "name": name, "design_print": print, "game_print": print }).to_string();
        let (status, _, _) = send(app, "POST", "/api/sync/done", Some(done)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        (Blueprint::from_sment(&bytes).unwrap(), print)
    }

    async fn designs(app: &Router) -> Vec<Summary> {
        let (_, _, bytes) = send(app, "GET", "/api/designs", None).await;
        serde_json::from_slice(&bytes).unwrap()
    }

    fn fetches(plan: &SyncPlan) -> Vec<(&str, &str)> {
        for f in &plan.fetch {
            assert_eq!(f.print.len(), 64, "a fetch names the version to fetch");
        }
        plan.fetch.iter().map(|f| (f.design.as_str(), f.name.as_str())).collect()
    }

    #[tokio::test]
    async fn sync_now_sends_a_design_without_waiting_for_it_to_settle() {
        let Some((_, _, cleanup)) = test_app("rush").await else { return };
        // A server that waits an hour before a design goes to the game.
        let config = Config {
            starmade_dir: starmade_dir().unwrap(),
            data_dir: cleanup.0.clone(),
            static_dir: PathBuf::from("missing"),
            sync_token: None,
            sync_quiet_secs: 3600,
        };
        let state = AppState::load(config).await.unwrap();
        let app = router(state.clone());
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Scout"}"#.into())).await;
        let scout: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!(fetches(&report(&app, &[]).await), vec![]);

        // Edits not yet on disk are written and go with it.
        let (_, rev) = download(&app, &scout.id).await;
        step(&app, &scout.id, rev, vec![hull_row(1, 2)]).await;
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/sync/now", scout.id), None).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        assert_eq!(fetches(&report(&app, &[]).await), vec![(scout.id.as_str(), "Scout")]);
        let (in_game, print) = import_into_game(&app, &scout.id, "Scout").await;
        assert_eq!(in_game.root.entity.block_count(), 3);

        // In step now: nothing to send, and the next edit waits as usual.
        let (status, _, _) = send(&app, "POST", &format!("/api/designs/{}/sync/now", scout.id), None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (_, rev) = download(&app, &scout.id).await;
        step(&app, &scout.id, rev, vec![hull_row(4, 4)]).await;
        state.sessions.flush(&state.store, &state.catalog, true);
        assert_eq!(fetches(&report(&app, &[("Scout", &print)]).await), vec![]);

        // Nor is a design that is switched off sent.
        send(&app, "POST", &format!("/api/designs/{}/sync", scout.id), Some(r#"{"on":false}"#.into())).await;
        let (status, _, _) = send(&app, "POST", &format!("/api/designs/{}/sync/now", scout.id), None).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    /// A report from a game with one blueprint and one shipyard.
    async fn report_yard(app: &Router, game: &[(&str, &str)], yard: &yard_core::api::GameYard) -> SyncPlan {
        let blueprints = game.iter().map(|(name, print)| serde_json::json!({ "name": name, "print": print })).collect::<Vec<_>>();
        let body = serde_json::json!({ "server": "test game", "blueprints": blueprints, "yards": [yard] }).to_string();
        let (status, _, bytes) = send(app, "POST", "/api/sync/report", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn yard_status(app: &Router, id: &str) -> yard_core::api::YardLinkInfo {
        let (status, _, bytes) = send(app, "GET", &format!("/api/designs/{id}/yard"), None).await;
        assert_eq!(status, StatusCode::OK);
        serde_json::from_slice::<Option<yard_core::api::YardLinkInfo>>(&bytes).unwrap().expect("linked")
    }

    async fn take_from_yard(app: &Router, design: &str, uid: &str, sment: Vec<u8>) -> StatusCode {
        let print = crate::sync::print_of_sment(&sment).unwrap();
        let uri = format!("/api/sync/yards/take?design={design}&uid={uid}&print={print}");
        let request = Request::builder().method("PUT").uri(uri).body(Body::from(sment)).unwrap();
        app.clone().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn a_design_follows_a_design_in_a_shipyard() {
        use yard_core::api::{GameYard, SyncDirection, YardDesign, YardState};
        let Some((app, state, _cleanup)) = test_app("yard").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Arrow"}"#.into())).await;
        let arrow: Summary = serde_json::from_slice(&body).unwrap();
        report(&app, &[]).await;
        let (_, game_print) = import_into_game(&app, &arrow.id, "Arrow").await;
        let mut game = vec![("Arrow".to_string(), game_print.clone())];
        let mut yard = GameYard { id: "ENTITY_SPACESTATION_Home@123".into(), station: "Home".into(), at: [0, 2, 5], room: true, ..GameYard::default() };
        // Shipyards are offered once the game reports them.
        let (_, _, bytes) = send(&app, "GET", "/api/sync/yards", None).await;
        assert_eq!(serde_json::from_slice::<Vec<GameYard>>(&bytes).unwrap(), vec![]);
        let plan_with = |yard: GameYard, game: Vec<(String, String)>| {
            let app = app.clone();
            async move {
                let game: Vec<(&str, &str)> = game.iter().map(|(n, p)| (n.as_str(), p.as_str())).collect();
                report_yard(&app, &game, &yard).await
            }
        };
        plan_with(yard.clone(), game.clone()).await;

        // Linked to a new design in a free shipyard: it is made from the catalog's blueprint.
        let link = serde_json::json!({ "yard": yard.id, "uid": null, "keep": "here" }).to_string();
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/yard", arrow.id), Some(link)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let plan = plan_with(yard.clone(), game.clone()).await;
        assert_eq!(plan.yards.give.len(), 1);
        let give = &plan.yards.give[0];
        assert_eq!((give.replace.as_deref(), give.name.as_str(), give.blueprint.as_str(), give.print.as_str()), (None, "Arrow", "Arrow", game_print.as_str()));
        // Asked once, not at every report.
        assert!(plan_with(yard.clone(), game.clone()).await.yards.give.is_empty());
        let given = serde_json::json!({ "design": arrow.id, "uid": "ENTITY_SHIP_Arrow_1", "print": game_print }).to_string();
        let (status, _, _) = send(&app, "POST", "/api/sync/yards/given", Some(given)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // The game's copy of it holds the same blocks: nothing changes here.
        yard.state = YardState::Loaded;
        yard.loaded = Some("ENTITY_SHIP_Arrow_1".into());
        yard.designs = vec![YardDesign { uid: "ENTITY_SHIP_Arrow_1".into(), name: "Arrow".into() }];
        let plan = plan_with(yard.clone(), game.clone()).await;
        assert_eq!(plan.yards.watch, vec!["ENTITY_SHIP_Arrow_1"]);
        assert!(plan.yards.take.is_empty());
        let (mut copy, _) = download(&app, &arrow.id).await;
        for entity in copy.blocks_mut() {
            let keys: Vec<_> = entity.segments().map(|(k, _)| *k).collect();
            for key in keys {
                let old = entity.segment(key).unwrap();
                let segment = yard_core::entity::Segment { blocks: old.blocks.clone(), count: old.count, version: old.version, timestamp: 42 };
                entity.insert_segment(key, segment);
            }
        }
        copy.rename("ENTITY_SHIP_Arrow_1");
        let copy_file = copy.to_sment().unwrap();
        yard.loaded_print = crate::sync::print_of_sment(&copy_file);
        let plan = plan_with(yard.clone(), game.clone()).await;
        assert_eq!(plan.yards.take.len(), 1);
        assert_eq!(take_from_yard(&app, &arrow.id, "ENTITY_SHIP_Arrow_1", copy_file).await, StatusCode::NO_CONTENT);
        assert_eq!(state.store.summary(&arrow.id).unwrap().print, arrow.print);
        assert_eq!(plan_with(yard.clone(), game.clone()).await.yards, yard_core::api::YardPlan { watch: vec!["ENTITY_SHIP_Arrow_1".into()], ..Default::default() });
        assert!(yard_status(&app, &arrow.id).await.in_step);

        // Built on in the shipyard: comes here, and on to the catalog.
        let (built, built_print) = game_ship("ENTITY_SHIP_Arrow_1", 5);
        yard.loaded_print = Some(built_print);
        let plan = plan_with(yard.clone(), game.clone()).await;
        assert_eq!(plan.yards.take.len(), 1);
        assert_eq!(take_from_yard(&app, &arrow.id, "ENTITY_SHIP_Arrow_1", built).await, StatusCode::NO_CONTENT);
        let now = state.store.summary(&arrow.id).unwrap();
        assert_eq!((now.name.as_str(), now.blocks), ("Arrow", 6));
        assert_eq!(history(&app, &arrow.id).await[0].direction, SyncDirection::FromShipyard);
        let plan = plan_with(yard.clone(), game.clone()).await;
        assert_eq!(fetches(&plan), vec![(arrow.id.as_str(), "Arrow")]);
        assert!(plan.yards.give.is_empty(), "the shipyard has it already");
        let (_, game_print) = import_into_game(&app, &arrow.id, "Arrow").await;
        game = vec![("Arrow".to_string(), game_print.clone())];
        assert_eq!(plan_with(yard.clone(), game.clone()).await.yards.give, vec![]);

        // Edited here: once the catalog has it, it replaces the shipyard's design.
        let (_, rev) = download(&app, &arrow.id).await;
        step(&app, &arrow.id, rev, vec![hull_row(9, 9)]).await;
        state.sessions.flush(&state.store, &state.catalog, true);
        let plan = plan_with(yard.clone(), game.clone()).await;
        assert!(plan.yards.give.is_empty(), "not before the catalog has it");
        assert_eq!(fetches(&plan), vec![(arrow.id.as_str(), "Arrow")]);
        let (_, game_print) = import_into_game(&app, &arrow.id, "Arrow").await;
        game = vec![("Arrow".to_string(), game_print.clone())];
        let plan = plan_with(yard.clone(), game.clone()).await;
        assert_eq!(plan.yards.give.len(), 1);
        assert_eq!(plan.yards.give[0].replace.as_deref(), Some("ENTITY_SHIP_Arrow_1"));
        // The agent could not: the player sees why, and it is asked again.
        let trouble = serde_json::json!({ "design": arrow.id, "why": "the shipyard is out of power" }).to_string();
        send(&app, "POST", "/api/sync/yards/trouble", Some(trouble)).await;
        assert!(yard_status(&app, &arrow.id).await.status.contains("out of power"));
        // Asked again after a while, not at once.
        assert_eq!(plan_with(yard.clone(), game.clone()).await.yards.give, vec![]);
        state.sync.age_yard_attempts(&arrow.id, 60);
        assert_eq!(plan_with(yard.clone(), game.clone()).await.yards.give.len(), 1);

        // Changed on both sides: the agent does not swap out a design that
        // changed since it was last seen, the shipyard's comes here and the
        // edits here are kept apart.
        let trouble = serde_json::json!({ "design": arrow.id, "why": "it changed in the shipyard" }).to_string();
        send(&app, "POST", "/api/sync/yards/trouble", Some(trouble)).await;
        let (both, both_print) = game_ship("ENTITY_SHIP_Arrow_1", 2);
        yard.loaded_print = Some(both_print);
        assert_eq!(plan_with(yard.clone(), game.clone()).await.yards.take.len(), 1);
        assert_eq!(take_from_yard(&app, &arrow.id, "ENTITY_SHIP_Arrow_1", both).await, StatusCode::NO_CONTENT);
        assert_eq!(state.store.summary(&arrow.id).unwrap().blocks, 3);
        let names: Vec<String> = designs(&app).await.into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"Arrow shipyard copy".to_string()), "{names:?}");

        // Linked again, keeping the shipyard's: it holds what the design
        // here holds, so nothing changes and nothing is set aside.
        send(&app, "DELETE", &format!("/api/designs/{}/yard", arrow.id), None).await;
        let link = serde_json::json!({ "yard": yard.id, "uid": "ENTITY_SHIP_Arrow_1", "keep": "shipyard" }).to_string();
        let (status, _, _) = send(&app, "POST", &format!("/api/designs/{}/yard", arrow.id), Some(link)).await;
        assert_eq!(status, StatusCode::OK);
        let (mut same, _) = download(&app, &arrow.id).await;
        same.rename("ENTITY_SHIP_Arrow_1");
        let same = same.to_sment().unwrap();
        yard.loaded_print = crate::sync::print_of_sment(&same);
        assert_eq!(plan_with(yard.clone(), game.clone()).await.yards.take.len(), 1);
        let before = (state.store.summary(&arrow.id).unwrap().print, designs(&app).await.len());
        assert_eq!(take_from_yard(&app, &arrow.id, "ENTITY_SHIP_Arrow_1", same).await, StatusCode::NO_CONTENT);
        assert_eq!((state.store.summary(&arrow.id).unwrap().print, designs(&app).await.len()), before);
        assert!(yard_status(&app, &arrow.id).await.in_step);

        // An upload for a design that is not the linked one is turned away.
        let (other, _) = game_ship("x", 1);
        assert_ne!(take_from_yard(&app, &arrow.id, "ENTITY_SHIP_other", other).await, StatusCode::NO_CONTENT);
        // Unlinked, both keep what they hold and nothing more moves.
        let (status, _, _) = send(&app, "DELETE", &format!("/api/designs/{}/yard", arrow.id), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(plan_with(yard.clone(), game.clone()).await.yards, yard_core::api::YardPlan::default());
    }

    #[tokio::test]
    async fn designs_and_game_blueprints_follow_each_other() {
        use yard_core::api::SyncStatus;
        let Some((app, state, _cleanup)) = test_app("sync").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Scout"}"#.into())).await;
        let scout: Summary = serde_json::from_slice(&body).unwrap();
        // Before any game server has called, designs say nothing about syncing.
        assert_eq!(scout.sync, None);
        let (raider_file, raider_print) = game_ship("Raider", 3);

        // First contact: the game's blueprint is wanted here, the design there.
        let plan = report(&app, &[("Raider", &raider_print)]).await;
        assert_eq!(plan.send, vec!["Raider"]);
        assert_eq!(fetches(&plan), vec![(scout.id.as_str(), "Scout")]);

        let (status, body) = put_blueprint(&app, "Raider", &raider_print, raider_file.clone()).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let raider: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!((raider.name.as_str(), raider.blocks), ("Raider", 4));
        // An untouched, current-format blueprint is stored exactly as it came.
        assert_eq!(state.store.file(&raider.id).unwrap(), raider_file);
        let (in_game, scout_print) = import_into_game(&app, &scout.id, "Scout").await;
        assert_eq!(in_game.name, "Scout");

        // Now both sides hold both, and nothing moves.
        let both = [("Raider", raider_print.as_str()), ("Scout", scout_print.as_str())];
        assert_eq!(report(&app, &both).await, SyncPlan::default());
        assert!(designs(&app).await.iter().all(|d| d.sync == Some(SyncStatus::Synced)));

        // An edit here goes to the game once it is saved.
        let (_, rev) = download(&app, &scout.id).await;
        let (status, _) = step(&app, &scout.id, rev, vec![hull_row(1, 2)]).await;
        assert_eq!(status, StatusCode::OK);
        state.flush(true);
        assert_eq!(designs(&app).await.iter().find(|d| d.id == scout.id).unwrap().sync, Some(SyncStatus::Waiting));
        let plan = report(&app, &both).await;
        assert_eq!((plan.send.len(), fetches(&plan)), (0, vec![(scout.id.as_str(), "Scout")]));
        let (in_game, scout_print) = import_into_game(&app, &scout.id, "Scout").await;
        assert_eq!(in_game.root.entity.block_count(), 3);

        // A new save in the game replaces the design here, under the same id.
        let (raider_file, raider_print) = game_ship("Raider", 6);
        let both = [("Raider", raider_print.as_str()), ("Scout", scout_print.as_str())];
        assert_eq!(report(&app, &both).await, SyncPlan { send: vec!["Raider".into()], ..SyncPlan::default() });
        let (status, body) = put_blueprint(&app, "Raider", &raider_print, raider_file).await;
        assert_eq!(status, StatusCode::OK);
        let updated: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!((updated.id.as_str(), updated.blocks, updated.created), (raider.id.as_str(), 7, raider.created));
        // Editors that have it open can tell it was replaced under them; an
        // edit of their own does not look like that.
        assert_eq!(updated.epoch, raider.epoch + 1);
        let scout_now = designs(&app).await.into_iter().find(|d| d.id == scout.id).unwrap();
        assert_eq!((scout_now.blocks, scout_now.epoch), (3, scout.epoch));
        assert_eq!(report(&app, &both).await, SyncPlan::default());
        assert_eq!(designs(&app).await.len(), 2);
    }

    #[tokio::test]
    async fn edits_made_here_survive_a_newer_save_in_the_game() {
        let Some((app, state, _cleanup)) = test_app("clash").await else { return };
        let (file, print) = game_ship("Raider", 3);
        assert_eq!(report(&app, &[("Raider", &print)]).await.send, vec!["Raider"]);
        let (_, body) = put_blueprint(&app, "Raider", &print, file).await;
        let raider: Summary = serde_json::from_slice(&body).unwrap();

        // Someone edits it here, and the edit has not even reached the disk
        // when a newer version arrives from the game.
        let (_, rev) = download(&app, &raider.id).await;
        let far_row = Op::Fill {
            min: Pos::new(0, 5, 0),
            max: Pos::new(9, 5, 0),
            block: Block::new(5, 0, false),
            mode: FillMode::Solid,
            when: When::Empty,
            link: None,
        };
        let (status, _) = step(&app, &raider.id, rev, vec![far_row]).await;
        assert_eq!(status, StatusCode::OK);
        let (newer, newer_print) = game_ship("Raider", 8);
        assert_eq!(report(&app, &[("Raider", &newer_print)]).await.send, vec!["Raider"]);
        let (status, _) = put_blueprint(&app, "Raider", &newer_print, newer).await;
        assert_eq!(status, StatusCode::OK);

        // The design is the game's version; the edit lives on as a copy.
        let all = designs(&app).await;
        assert_eq!(all.len(), 2);
        let (bp, new_rev) = download(&app, &raider.id).await;
        assert_eq!(bp.root.entity.block_count(), 9);
        let copy = all.iter().find(|d| d.id != raider.id).unwrap();
        assert_eq!((copy.name.as_str(), copy.blocks), ("Raider shipyard copy", 14));
        let on_disk = Blueprint::from_sment(&state.store.file(&copy.id).unwrap()).unwrap();
        assert_eq!(on_disk.name, "Raider shipyard copy");
        // The editor that made the edit is out of step now and will reload.
        let (status, _) = step(&app, &raider.id, rev + 1, vec![hull_row(20, 21)]).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_ne!(new_rev, rev + 1);
        // The copy goes to the game as a blueprint of its own.
        let plan = report(&app, &[("Raider", &newer_print)]).await;
        assert_eq!((plan.send.len(), fetches(&plan)), (0, vec![(copy.id.as_str(), "Raider shipyard copy")]));
    }

    #[tokio::test]
    async fn deleting_in_the_game_and_switching_off_stop_the_exchange() {
        use yard_core::api::SyncStatus;
        let Some((app, _, _cleanup)) = test_app("gone").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Scout"}"#.into())).await;
        let scout: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!(fetches(&report(&app, &[]).await), vec![(scout.id.as_str(), "Scout")]);
        let (_, print) = import_into_game(&app, &scout.id, "Scout").await;
        assert_eq!(report(&app, &[("Scout", &print)]).await, SyncPlan::default());

        // An admin deletes the blueprint in the game. It is not pushed back.
        assert_eq!(report(&app, &[]).await, SyncPlan::default());
        assert_eq!(designs(&app).await[0].sync, Some(SyncStatus::Gone));
        assert_eq!(report(&app, &[]).await, SyncPlan::default());
        // Had it only been out of sight for a moment, all would be as before.
        assert_eq!(report(&app, &[("Scout", &print)]).await, SyncPlan::default());
        assert_eq!(designs(&app).await[0].sync, Some(SyncStatus::Synced));
        assert_eq!(report(&app, &[]).await, SyncPlan::default());

        // Switching it on again sends it as a new blueprint.
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/sync", scout.id), Some(r#"{"on":true}"#.into())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(serde_json::from_slice::<Summary>(&body).unwrap().sync, Some(SyncStatus::Waiting));
        assert_eq!(fetches(&report(&app, &[]).await), vec![(scout.id.as_str(), "Scout")]);

        // Switched off by hand, a design stays put whatever the game does.
        let (in_game, print) = import_into_game(&app, &scout.id, "Scout").await;
        send(&app, "POST", &format!("/api/designs/{}/sync", scout.id), Some(r#"{"on":false}"#.into())).await;
        assert_eq!(report(&app, &[]).await, SyncPlan::default());
        let (newer, newer_print) = game_ship("Scout", 9);
        assert_eq!(report(&app, &[("Scout", &newer_print)]).await, SyncPlan::default());
        // Even an agent that sends the new version unasked changes nothing.
        let (status, _) = put_blueprint(&app, "Scout", &newer_print, newer).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (kept, _) = download(&app, &scout.id).await;
        assert_eq!(kept.root.entity.block_count(), in_game.root.entity.block_count());
        assert_eq!(report(&app, &[("Scout", &print)]).await, SyncPlan::default());

        // A second design may not take the name of the first.
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"scout"}"#.into())).await;
        assert_eq!(serde_json::from_slice::<Summary>(&body).unwrap().name, "scout 2");
    }

    #[tokio::test]
    async fn a_renamed_design_goes_to_the_game_under_its_new_name() {
        let Some((app, state, _cleanup)) = test_app("renamed").await else { return };
        let rename = |id: &str, name: &str| {
            let (app, uri, body) = (app.clone(), format!("/api/designs/{id}/rename"), serde_json::json!({ "name": name }).to_string());
            async move { send(&app, "POST", &uri, Some(body)).await }
        };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Scout"}"#.into())).await;
        let scout: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!(fetches(&report(&app, &[]).await), vec![(scout.id.as_str(), "Scout")]);
        let (_, old_print) = import_into_game(&app, &scout.id, "Scout").await;
        assert_eq!(report(&app, &[("Scout", &old_print)]).await, SyncPlan::default());

        // Names the game would not take, and capitals alone on a design the game has, are refused.
        assert_eq!(rename(&scout.id, "!!!").await.0, StatusCode::BAD_REQUEST);
        assert_eq!(rename(&scout.id, "scout").await.0, StatusCode::CONFLICT);
        assert_eq!(rename("nonesuch", "Anything").await.0, StatusCode::NOT_FOUND);

        let (status, _, body) = rename(&scout.id, "Scout Mk2").await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let renamed: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!((renamed.id.as_str(), renamed.name.as_str()), (scout.id.as_str(), "Scout Mk2"));
        assert_eq!(Blueprint::from_sment(&state.store.file(&scout.id).unwrap()).unwrap().name, "Scout Mk2");

        // The game's "Scout" stays there and is not taken in as a copy; the
        // design goes to the game under its new name.
        let plan = report(&app, &[("Scout", &old_print)]).await;
        assert_eq!((plan.send.len(), fetches(&plan)), (0, vec![(scout.id.as_str(), "Scout Mk2")]));
        // An import under the old name that finishes only now does not link
        // the two again, and the game's "Scout" it overwrote is not taken in.
        let (_, late_print) = import_into_game(&app, &scout.id, "Scout").await;
        let plan = report(&app, &[("Scout", &late_print)]).await;
        assert_eq!((plan.send.len(), fetches(&plan)), (0, vec![(scout.id.as_str(), "Scout Mk2")]));
        let (_, new_print) = import_into_game(&app, &scout.id, "Scout Mk2").await;
        assert_eq!(report(&app, &[("Scout", &late_print), ("Scout Mk2", &new_print)]).await, SyncPlan::default());
        assert_eq!(designs(&app).await.len(), 1);

        // No two designs share a name.
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Other"}"#.into())).await;
        let other: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!(rename(&other.id, "scout mk2").await.0, StatusCode::CONFLICT);
        // A design the game has never had may change its capitals.
        let (status, _, body) = rename(&other.id, "OTHER").await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    }

    #[tokio::test]
    async fn a_design_named_like_a_different_game_blueprint_is_renamed() {
        let Some((app, _, _cleanup)) = test_app("rename").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Raider"}"#.into())).await;
        let mine: Summary = serde_json::from_slice(&body).unwrap();
        let (file, print) = game_ship("Raider", 3);
        // The design steps aside; the game's blueprint is asked for.
        let plan = report(&app, &[("Raider", &print)]).await;
        assert_eq!((plan.send.clone(), plan.fetch.len()), (vec!["Raider".to_string()], 0));
        let (bp, _) = download(&app, &mine.id).await;
        assert_eq!(bp.name, "Raider 2");
        let (status, _) = put_blueprint(&app, "Raider", &print, file).await;
        assert_eq!(status, StatusCode::OK);
        let plan = report(&app, &[("Raider", &print)]).await;
        assert_eq!((plan.send.len(), fetches(&plan)), (0, vec![(mine.id.as_str(), "Raider 2")]));
    }

    #[tokio::test]
    async fn two_designs_that_step_aside_at_once_get_different_names() {
        let Some((app, state, _cleanup)) = test_app("rename2").await else { return };
        // Two designs here are called like a blueprint in the game. (Made
        // directly in the store: the site itself never lets two share a name.)
        for _ in 0..2 {
            let bp = Blueprint::new_ship("Raider", "test");
            let id = Store::new_id();
            state.store.save(Summary::of(&id, &bp, &state.catalog, 1), &bp.to_sment().unwrap()).unwrap();
        }
        let (_, print) = game_ship("Raider", 3);
        report(&app, &[("Raider", &print)]).await;
        let mut names: Vec<String> = designs(&app).await.into_iter().map(|d| d.name).collect();
        names.sort();
        assert_eq!(names, vec!["Raider 2", "Raider 3"]);
    }

    #[tokio::test]
    async fn a_torn_or_unreadable_upload_from_the_game_is_refused() {
        let Some((app, _, _cleanup)) = test_app("torn").await else { return };
        let (file, print) = game_ship("Raider", 3);
        let (_, other_print) = game_ship("Raider", 4);
        let (status, _) = put_blueprint(&app, "Raider", &other_print, file.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = put_blueprint(&app, "Raider", &print, b"not a zip".to_vec()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        // That version is not asked for again; the next one is.
        assert_eq!(report(&app, &[("Raider", &print)]).await, SyncPlan::default());
        assert_eq!(report(&app, &[("Raider", &other_print)]).await.send, vec!["Raider"]);
        let (status, _) = put_blueprint(&app, "..%2Fx", &print, file).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(designs(&app).await.is_empty());
        let (status, _, _) = send(&app, "GET", "/api/sync/designs/aaaaaaaaaaaa/file", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    async fn history(app: &Router, id: &str) -> Vec<SyncVersion> {
        let (status, _, bytes) = send(app, "GET", &format!("/api/designs/{id}/history"), None).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Sequence number, direction, blocks and whether it is the current one.
    fn outline(versions: &[SyncVersion]) -> Vec<(u64, yard_core::api::SyncDirection, u64, bool)> {
        versions.iter().map(|v| (v.seq, v.direction, v.blocks, v.current)).collect()
    }

    #[tokio::test]
    async fn every_synced_version_is_kept_and_can_be_read_back() {
        use yard_core::api::SyncDirection::{FromGame, Matched, ToGame};
        let Some((app, state, _cleanup)) = test_app("history").await else { return };
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Scout"}"#.into())).await;
        let scout: Summary = serde_json::from_slice(&body).unwrap();
        // Nothing has been synced yet.
        assert!(history(&app, &scout.id).await.is_empty());

        // From the game: the version that arrived.
        let (raider_file, raider_print) = game_ship("Raider", 3);
        report(&app, &[("Raider", &raider_print)]).await;
        let (_, body) = put_blueprint(&app, "Raider", &raider_print, raider_file.clone()).await;
        let raider: Summary = serde_json::from_slice(&body).unwrap();
        let versions = history(&app, &raider.id).await;
        assert_eq!(outline(&versions), vec![(1, FromGame, 4, true)]);
        assert_eq!((versions[0].name.as_str(), versions[0].entities, versions[0].bytes), ("Raider", 1, raider_file.len() as u64));
        assert_eq!(versions[0].print, raider.print);
        assert!(versions[0].time >= raider.created);

        // To the game: the version the game confirmed, kept even though the
        // design is edited again before the confirmation arrives.
        let (status, headers, sent) = send(&app, "GET", &format!("/api/sync/designs/{}/file", scout.id), None).await;
        assert_eq!(status, StatusCode::OK);
        let sent_print = headers.get(PRINT_HEADER).unwrap().to_str().unwrap().to_string();
        assert!(history(&app, &scout.id).await.is_empty(), "not a synced version until the game has it");
        let (_, rev) = download(&app, &scout.id).await;
        let (status, _) = step(&app, &scout.id, rev, vec![hull_row(1, 2)]).await;
        assert_eq!(status, StatusCode::OK);
        state.flush(true);
        let done = serde_json::json!({ "design": scout.id, "name": "Scout", "design_print": sent_print, "game_print": sent_print }).to_string();
        let (status, _, _) = send(&app, "POST", "/api/sync/done", Some(done)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // The design has moved on, so no version is the current one.
        assert_eq!(outline(&history(&app, &scout.id).await), vec![(1, ToGame, 1, false)]);
        let (status, headers, kept) = send(&app, "GET", &format!("/api/designs/{}/history/1/file", scout.id), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(kept, sent);
        assert_eq!(headers.get(header::CONTENT_DISPOSITION).unwrap(), "attachment; filename=\"Scout v1.sment\"");
        assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "application/zip");
        assert_eq!(Blueprint::from_sment(&kept).unwrap().root.entity.block_count(), 1);
        assert_eq!(crate::sync::print_of_sment(&kept).unwrap(), sent_print);

        // The edit follows, and the list is newest first.
        let (_, scout_print) = import_into_game(&app, &scout.id, "Scout").await;
        let versions = history(&app, &scout.id).await;
        assert_eq!(outline(&versions), vec![(2, ToGame, 3, true), (1, ToGame, 1, false)]);
        assert_eq!(versions[0].print, scout_print);
        // Confirming the same version again adds nothing.
        import_into_game(&app, &scout.id, "Scout").await;
        assert_eq!(history(&app, &scout.id).await.len(), 2);

        // A newer save in the game is the next version of its design.
        let (newer_file, newer_print) = game_ship("Raider", 6);
        let (status, _) = put_blueprint(&app, "Raider", &newer_print, newer_file.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(outline(&history(&app, &raider.id).await), vec![(2, FromGame, 7, true), (1, FromGame, 4, false)]);
        let (_, _, first) = send(&app, "GET", &format!("/api/designs/{}/history/1/file", raider.id), None).await;
        assert_eq!(first, raider_file, "the earlier version is still what it was");
        assert_eq!(state.store.file(&raider.id).unwrap(), newer_file);

        // The same ship on both sides from the start: paired up, and kept.
        let (twin_file, twin_print) = game_ship("Twin", 2);
        let request = Request::builder().method("POST").uri("/api/designs?name=Twin.sment").body(Body::from(twin_file)).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let twin: Summary = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let game = [("Raider", newer_print.as_str()), ("Scout", scout_print.as_str()), ("Twin", twin_print.as_str())];
        assert_eq!(report(&app, &game).await, SyncPlan::default());
        assert_eq!(outline(&history(&app, &twin.id).await), vec![(1, Matched, 3, true)]);

        // What is not there is not found.
        for uri in [
            "/api/designs/aaaaaaaaaaaa/history".to_string(),
            "/api/designs/..%2F..%2Fx/history".to_string(),
            format!("/api/designs/{}/history/9/file", scout.id),
            "/api/designs/aaaaaaaaaaaa/history/1/file".to_string(),
        ] {
            let (status, _, _) = send(&app, "GET", &uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        }
        for action in ["restore", "copy"] {
            let (status, _, _) = send(&app, "POST", &format!("/api/designs/{}/history/9/{action}", scout.id), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _, _) = send(&app, "POST", &format!("/api/designs/aaaaaaaaaaaa/history/1/{action}"), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn an_earlier_version_can_be_put_back_or_stored_as_a_design_of_its_own() {
        use yard_core::api::SyncDirection::{FromGame, ToGame};
        use yard_core::api::SyncStatus;
        let Some((app, state, _cleanup)) = test_app("restore").await else { return };
        let (first_file, first_print) = game_ship("Raider", 3);
        report(&app, &[("Raider", &first_print)]).await;
        let (_, body) = put_blueprint(&app, "Raider", &first_print, first_file.clone()).await;
        let raider: Summary = serde_json::from_slice(&body).unwrap();
        let (second_file, second_print) = game_ship("Raider", 6);
        let (_, body) = put_blueprint(&app, "Raider", &second_print, second_file).await;
        let second: Summary = serde_json::from_slice(&body).unwrap();
        let game = [("Raider", second_print.as_str())];
        assert_eq!(report(&app, &game).await, SyncPlan::default());

        // Someone has the design open when it is put back to its first version.
        let (_, rev) = download(&app, &raider.id).await;
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/history/1/restore", raider.id), None).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let restored: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!((restored.id.as_str(), restored.name.as_str(), restored.blocks), (raider.id.as_str(), "Raider", 4));
        assert_eq!((restored.print.as_str(), restored.created), (raider.print.as_str(), raider.created));
        // Their editor is told to load it again, and the game is to be told.
        assert_eq!((restored.epoch, restored.sync), (second.epoch + 1, Some(SyncStatus::Waiting)));
        let (status, _) = step(&app, &raider.id, rev, vec![hull_row(20, 21)]).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(state.store.file(&raider.id).unwrap(), first_file);
        let (bp, _) = download(&app, &raider.id).await;
        assert_eq!(bp.root.entity.block_count(), 4);
        assert_eq!(outline(&history(&app, &raider.id).await), vec![(2, FromGame, 7, false), (1, FromGame, 4, true)]);

        // The game still has the second version, so the first goes there like any edit.
        let plan = report(&app, &game).await;
        assert_eq!((plan.send.len(), fetches(&plan)), (0, vec![(raider.id.as_str(), "Raider")]));
        let (in_game, print) = import_into_game(&app, &raider.id, "Raider").await;
        assert_eq!((in_game.root.entity.block_count(), print.as_str()), (4, first_print.as_str()));
        assert_eq!(report(&app, &[("Raider", &print)]).await, SyncPlan::default());
        assert_eq!(outline(&history(&app, &raider.id).await), vec![(3, ToGame, 4, true), (2, FromGame, 7, false), (1, FromGame, 4, true)]);

        // A version as a design of its own: named after it, and kept out of the game.
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/history/2/copy", raider.id), None).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let copy: Summary = serde_json::from_slice(&body).unwrap();
        assert_ne!(copy.id, raider.id);
        assert_eq!((copy.name.as_str(), copy.blocks, copy.sync), ("Raider v2", 7, Some(SyncStatus::Off)));
        assert_eq!(Blueprint::from_sment(&state.store.file(&copy.id).unwrap()).unwrap().name, "Raider v2");
        assert_eq!(report(&app, &[("Raider", &print)]).await, SyncPlan::default());
        assert!(history(&app, &copy.id).await.is_empty());
        let (_, _, body) = send(&app, "POST", &format!("/api/designs/{}/history/2/copy", raider.id), None).await;
        assert_eq!(serde_json::from_slice::<Summary>(&body).unwrap().name, "Raider v2 2");
        // The design itself is untouched by copying.
        assert_eq!(state.store.summary(&raider.id).unwrap().print, first_print);

        // A design that has been given another name since keeps that name when
        // an earlier version is put back: the name is what pairs it with the game.
        assert!(state.sessions.rename(&state.store, &state.catalog, &raider.id, "Marauder").is_ok());
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/history/2/restore", raider.id), None).await;
        assert_eq!(status, StatusCode::OK);
        let renamed: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!((renamed.name.as_str(), renamed.blocks), ("Marauder", 7));
        assert_eq!(Blueprint::from_sment(&state.store.file(&raider.id).unwrap()).unwrap().name, "Marauder");

        // A long name makes room for the version number.
        let long = "A".repeat(MAX_NAME);
        let (file, print) = game_ship(&long, 1);
        let (status, body) = put_blueprint(&app, &long, &print, file).await;
        assert_eq!(status, StatusCode::OK);
        let long_design: Summary = serde_json::from_slice(&body).unwrap();
        let (_, _, body) = send(&app, "POST", &format!("/api/designs/{}/history/1/copy", long_design.id), None).await;
        let copy: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!(copy.name, format!("{} v1", "A".repeat(MAX_NAME - 3)));
        assert!(valid_name(&copy.name));
    }

    #[tokio::test]
    async fn putting_a_version_back_keeps_the_edits_it_replaces() {
        use yard_core::api::SyncDirection::{FromGame, ToGame, Unsynced};
        let Some((app, state, _cleanup)) = test_app("unsynced").await else { return };
        let (file, first_print) = game_ship("Raider", 3);
        report(&app, &[("Raider", &first_print)]).await;
        let (_, body) = put_blueprint(&app, "Raider", &first_print, file).await;
        let raider: Summary = serde_json::from_slice(&body).unwrap();
        let (file, second_print) = game_ship("Raider", 6);
        put_blueprint(&app, "Raider", &second_print, file).await;

        // Edits made here, not even on disk yet, when the first version is put back.
        let (_, rev) = download(&app, &raider.id).await;
        let (status, _) = step(&app, &raider.id, rev, vec![hull_row(7, 8)]).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/history/1/restore", raider.id), None).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        assert_eq!(serde_json::from_slice::<Summary>(&body).unwrap().blocks, 4);
        // They are a version now, the newest, marked as never synced.
        let versions = history(&app, &raider.id).await;
        assert_eq!(outline(&versions), vec![(3, Unsynced, 9, false), (2, FromGame, 7, false), (1, FromGame, 4, true)]);
        let (_, _, kept) = send(&app, "GET", &format!("/api/designs/{}/history/3/file", raider.id), None).await;
        let edited = Blueprint::from_sment(&kept).unwrap();
        assert_eq!(edited.root.entity.block_count(), 9);
        assert_eq!(edited.root.entity.get(Pos::new(8, 0, 0)).ty(), 5);

        // And they can be put back in turn. What that replaces is a kept
        // version already, so nothing is added.
        let (status, _, body) = send(&app, "POST", &format!("/api/designs/{}/history/3/restore", raider.id), None).await;
        assert_eq!(status, StatusCode::OK);
        let back: Summary = serde_json::from_slice(&body).unwrap();
        assert_eq!((back.blocks, back.print.as_str()), (9, versions[0].print.as_str()));
        let (bp, _) = download(&app, &raider.id).await;
        assert_eq!(bp.root.entity.get(Pos::new(8, 0, 0)).ty(), 5);
        assert_eq!(outline(&history(&app, &raider.id).await), vec![(3, Unsynced, 9, true), (2, FromGame, 7, false), (1, FromGame, 4, false)]);

        // Once the game has them, they are no longer edits it never saw.
        let plan = report(&app, &[("Raider", &second_print)]).await;
        assert_eq!(fetches(&plan), vec![(raider.id.as_str(), "Raider")]);
        import_into_game(&app, &raider.id, "Raider").await;
        assert_eq!(outline(&history(&app, &raider.id).await), vec![(3, ToGame, 9, true), (2, FromGame, 7, false), (1, FromGame, 4, false)]);
        drop(state);
    }

    #[tokio::test]
    async fn designs_synced_before_the_history_existed_start_with_the_version_the_game_has() {
        use yard_core::api::SyncDirection::Matched;
        let Some((app, state, cleanup)) = test_app("seed").await else { return };
        let (file, print) = game_ship("Raider", 3);
        report(&app, &[("Raider", &print)]).await;
        let (_, body) = put_blueprint(&app, "Raider", &print, file.clone()).await;
        let raider: Summary = serde_json::from_slice(&body).unwrap();
        // A design that is not in step with the game has nothing to start from.
        let (_, _, body) = send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Scout"}"#.into())).await;
        let scout: Summary = serde_json::from_slice(&body).unwrap();
        drop((app, state));

        // The same data as a server from before the history would have left it.
        let data_dir = cleanup.0.clone();
        std::fs::remove_dir_all(data_dir.join("history")).unwrap();
        let reopen = || async {
            let config = Config {
                starmade_dir: starmade_dir().unwrap(),
                data_dir: data_dir.clone(),
                static_dir: PathBuf::from("missing"),
                sync_token: None,
                sync_quiet_secs: 0,
            };
            let state = AppState::load(config).await.unwrap();
            (router(state.clone()), state)
        };
        let (app, state) = reopen().await;
        let versions = history(&app, &raider.id).await;
        assert_eq!(outline(&versions), vec![(1, Matched, 4, true)]);
        assert_eq!((versions[0].time, versions[0].print.as_str()), (raider.modified, raider.print.as_str()));
        assert!(history(&app, &scout.id).await.is_empty());
        let kept = data_dir.join("history").join(&raider.id).join("1.sment");
        assert_eq!(std::fs::read(&kept).unwrap(), file);
        #[cfg(unix)]
        {
            // The same file under a second name, not a copy.
            use std::os::unix::fs::MetadataExt;
            let stored = data_dir.join("designs").join(format!("{}.sment", raider.id));
            assert_eq!(std::fs::metadata(&kept).unwrap().ino(), std::fs::metadata(&stored).unwrap().ino());
        }
        drop((app, state));
        // Starting again adds nothing.
        let (app, _state) = reopen().await;
        assert_eq!(history(&app, &raider.id).await, versions);
    }

    #[tokio::test]
    async fn the_agent_needs_the_secret_when_one_is_set() {
        let Some((app, _, _cleanup)) = test_app("secret").await else { return };
        let body = r#"{"server":"x","blueprints":[]}"#;
        let (status, _, _) = send(&app, "POST", "/api/sync/report", Some(body.into())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        for (given, expected) in [("wrong!", StatusCode::UNAUTHORIZED), ("s3cret", StatusCode::OK)] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/sync/report")
                .header(header::CONTENT_TYPE, "application/json")
                .header(TOKEN_HEADER, given)
                .body(Body::from(body))
                .unwrap();
            assert_eq!(app.clone().oneshot(request).await.unwrap().status(), expected);
        }
        // People using the editor are not asked for it.
        let (status, headers, _) = send(&app, "GET", "/api/designs", None).await;
        assert_eq!(status, StatusCode::OK);
        // And a list they already have is not sent twice.
        let tag = headers.get(header::ETAG).unwrap().clone();
        let again = Request::builder().uri("/api/designs").header(header::IF_NONE_MATCH, &tag).body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(again).await.unwrap().status(), StatusCode::NOT_MODIFIED);
        send(&app, "POST", "/api/designs/new", Some(r#"{"name":"Scout"}"#.into())).await;
        let again = Request::builder().uri("/api/designs").header(header::IF_NONE_MATCH, &tag).body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(again).await.unwrap().status(), StatusCode::OK);
    }
}
