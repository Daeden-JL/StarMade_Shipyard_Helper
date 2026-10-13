//! Calls to the shipyard server.

use gloo_net::http::{Request, Response};
use yard_core::api::{ApiError, ClientBlock, NewDesign, StepRequest, StepResponse, Summary, SyncInfo, SyncSwitch, REV_HEADER};

/// Turns a non-2xx response into the server's own sentence where it sent one.
async fn check(res: Response) -> Result<Response, String> {
    if res.ok() {
        return Ok(res);
    }
    let status = res.status();
    match res.json::<ApiError>().await {
        Ok(body) => Err(body.error),
        Err(_) => Err(format!("The server answered with an error ({status}).")),
    }
}

fn unreachable_server(e: gloo_net::Error) -> String {
    format!("Could not reach the server ({e}).")
}

pub async fn catalog() -> Result<Vec<ClientBlock>, String> {
    let res = check(Request::get("/api/catalog").send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

/// The 3D models the game draws for some blocks in place of a cube.
pub async fn models() -> Result<yard_core::model::ModelSet, String> {
    let res = check(Request::get("/api/models").send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

pub async fn designs() -> Result<Vec<Summary>, String> {
    let res = check(Request::get("/api/designs").send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

pub async fn design(id: &str) -> Result<Summary, String> {
    let res = check(Request::get(&format!("/api/designs/{id}")).send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

/// The design's blueprint file and the revision it is at.
pub async fn design_file(id: &str) -> Result<(Vec<u8>, u64), String> {
    let res = check(Request::get(&format!("/api/designs/{id}/file")).send().await.map_err(unreachable_server)?).await?;
    let rev = res
        .headers()
        .get(REV_HEADER)
        .and_then(|v| v.parse().ok())
        .ok_or("The server did not say which revision the design is at.")?;
    Ok((res.binary().await.map_err(|e| e.to_string())?, rev))
}

pub async fn upload(file_name: &str, bytes: &[u8]) -> Result<Summary, String> {
    let name = js_sys::encode_uri_component(file_name);
    let body = js_sys::Uint8Array::from(bytes);
    let res = Request::post(&format!("/api/designs?name={name}"))
        .header("Content-Type", "application/octet-stream")
        .body(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

pub async fn new_design(name: &str) -> Result<Summary, String> {
    let res = Request::post("/api/designs/new")
        .json(&NewDesign { name: name.to_string() })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

/// What the server knows about the game server it exchanges blueprints with.
pub async fn sync_info() -> Result<SyncInfo, String> {
    let res = check(Request::get("/api/sync").send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

/// Puts a design into the exchange with the game server or takes it out.
/// Gives a design a new name.
pub async fn rename_design(id: &str, name: &str) -> Result<Summary, String> {
    let res = Request::post(&format!("/api/designs/{id}/rename"))
        .json(&yard_core::api::Rename { name: name.to_string() })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

pub async fn switch_sync(id: &str, on: bool) -> Result<Summary, String> {
    let res = Request::post(&format!("/api/designs/{id}/sync"))
        .json(&SyncSwitch { on })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

pub async fn sync_now(id: &str) -> Result<Summary, String> {
    let res = Request::post(&format!("/api/designs/{id}/sync/now")).send().await.map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

pub async fn yard_link(id: &str) -> Result<Option<yard_core::api::YardLinkInfo>, String> {
    let res = check(Request::get(&format!("/api/designs/{id}/yard")).send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

pub async fn game_yards() -> Result<Vec<yard_core::api::GameYard>, String> {
    let res = check(Request::get("/api/sync/yards").send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

pub async fn link_yard(id: &str, request: &yard_core::api::YardLinkRequest) -> Result<Option<yard_core::api::YardLinkInfo>, String> {
    let res = Request::post(&format!("/api/designs/{id}/yard"))
        .json(request)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

pub async fn unlink_yard(id: &str) -> Result<(), String> {
    check(Request::delete(&format!("/api/designs/{id}/yard")).send().await.map_err(unreachable_server)?).await?;
    Ok(())
}

/// Why the server did not apply a step.
pub enum StepFailure {
    /// The step itself was turned down (nothing to undo, too large). The
    /// server's copy of the design is unchanged.
    Refused(String),
    /// This page's copy no longer matches the server's, or the server could not
    /// be asked. The design has to be loaded again.
    OutOfStep(String),
}

pub async fn step(id: &str, request: &StepRequest) -> Result<StepResponse, StepFailure> {
    let lost = |e: String| StepFailure::OutOfStep(e);
    let res = Request::post(&format!("/api/designs/{id}/steps"))
        .json(request)
        .map_err(|e| lost(e.to_string()))?
        .send()
        .await
        .map_err(|e| lost(unreachable_server(e)))?;
    let refused = res.status() == 422;
    match check(res).await {
        Ok(res) => res.json().await.map_err(|e| lost(e.to_string())),
        Err(e) if refused => Err(StepFailure::Refused(e)),
        Err(e) => Err(lost(e)),
    }
}

/// Where one version from a design's sync history can be downloaded.
pub fn version_file_url(id: &str, seq: u64) -> String {
    format!("/api/designs/{id}/history/{seq}/file")
}

/// The versions of a design that were in step with the game server, newest
/// first. If none is marked current, the design has changes the game has not
/// seen.
pub async fn sync_history(id: &str) -> Result<Vec<yard_core::api::SyncVersion>, String> {
    let res = check(Request::get(&format!("/api/designs/{id}/history")).send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

/// One version's blueprint file.
pub async fn version_file(id: &str, seq: u64) -> Result<Vec<u8>, String> {
    let res = check(Request::get(&version_file_url(id, seq)).send().await.map_err(unreachable_server)?).await?;
    res.binary().await.map_err(|e| e.to_string())
}

/// Makes a version the design's contents again. The design keeps its name.
/// If the design has edits the game never saw (no version is marked current),
/// the server keeps those as a new version first, so nothing is lost.
pub async fn restore_version(id: &str, seq: u64) -> Result<Summary, String> {
    let res = Request::post(&format!("/api/designs/{id}/history/{seq}/restore")).send().await.map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

/// Stores a version as a new design, which starts with syncing switched off.
pub async fn copy_version(id: &str, seq: u64) -> Result<Summary, String> {
    let res = Request::post(&format!("/api/designs/{id}/history/{seq}/copy")).send().await.map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

/// What the player chose for turning a 3D model into a ship.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportChoices {
    pub longest: u32,
    pub solid: bool,
    /// How thick the shell of a hollow result is, in blocks.
    pub shell: u8,
    /// "auto", "y" or "z".
    pub up: String,
    pub turn: u8,
    pub mirror: bool,
    pub smooth: bool,
    /// How tall a step the edges of are beveled, in blocks; 0 for none.
    pub step: u8,
}

/// Turns a 3D model file into a new ship on the server.
pub async fn import_model(file_name: &str, bytes: &[u8], choices: &ImportChoices) -> Result<yard_core::api::ImportResult, String> {
    let name = js_sys::encode_uri_component(file_name);
    let uri = format!(
        "/api/designs/import?name={name}&longest={}&solid={}&shell={}&up={}&turn={}&mirror={}&smooth={}&step={}",
        choices.longest, choices.solid, choices.shell, choices.up, choices.turn, choices.mirror, choices.smooth, choices.step
    );
    let res = Request::post(&uri)
        .header("Content-Type", "application/octet-stream")
        .body(js_sys::Uint8Array::from(bytes))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(unreachable_server)?;
    check(res).await?.json().await.map_err(|e| e.to_string())
}

/// A model the assistant can run on.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct ChatModel {
    pub id: String,
    pub name: String,
    /// "claude" or "openai"
    pub provider: String,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ChatModels {
    pub models: Vec<ChatModel>,
    pub notes: Vec<String>,
}

pub async fn chat_models() -> Result<ChatModels, String> {
    let res = check(Request::get("/api/chat/models").send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

/// Gives the assistant a message to work on.
pub async fn chat_ask(id: &str, model: &ChatModel, message: &str, effort: Option<&str>, entity: &str) -> Result<(), String> {
    let body = serde_json::json!({"provider": model.provider, "model": model.id, "message": message, "effort": effort, "entity": entity});
    let res = Request::post(&format!("/api/designs/{id}/chat")).json(&body).map_err(|e| e.to_string())?.send().await.map_err(unreachable_server)?;
    check(res).await.map(|_| ())
}

/// What the assistant has done since entry `from`: the next entry to ask
/// for, whether it is still working, and the entries.
pub async fn chat_log(id: &str, from: usize) -> Result<serde_json::Value, String> {
    let res = check(Request::get(&format!("/api/designs/{id}/chat?from={from}")).send().await.map_err(unreachable_server)?).await?;
    res.json().await.map_err(|e| e.to_string())
}

pub async fn chat_stop(id: &str) -> Result<(), String> {
    let res = Request::post(&format!("/api/designs/{id}/chat/stop")).send().await.map_err(unreachable_server)?;
    check(res).await.map(|_| ())
}

pub async fn chat_reset(id: &str) -> Result<(), String> {
    let res = Request::delete(&format!("/api/designs/{id}/chat")).send().await.map_err(unreachable_server)?;
    check(res).await.map(|_| ())
}

/// Deletes a design. The server keeps its files in a trash folder.
pub async fn delete_design(id: &str) -> Result<(), String> {
    let res = Request::delete(&format!("/api/designs/{id}")).send().await.map_err(|e| e.to_string())?;
    check(res).await.map(|_| ())
}

/// Sends a change of structure. Turned down because this page's copy is
/// behind the server's, it fails as out of step: the design has to be loaded
/// again before the change can be asked for.
async fn post_structure<B: serde::Serialize>(uri: &str, body: &B) -> Result<yard_core::api::Restructured, StepFailure> {
    let refused = |e: String| StepFailure::Refused(e);
    let res = Request::post(uri).json(body).map_err(|e| refused(e.to_string()))?.send().await.map_err(|e| refused(unreachable_server(e)))?;
    let behind = res.status() == 409;
    match check(res).await {
        Ok(res) => res.json().await.map_err(|e| refused(e.to_string())),
        Err(e) if behind => Err(StepFailure::OutOfStep(e)),
        Err(e) => Err(refused(e)),
    }
}

/// Moves the core of an entity to another cell; the whole design shifts round it.
/// With `facing` (the ways that are to become forward and up), the entity also turns round the core.
/// With `dock_follows`, on a turret, the dock goes with the core.
pub async fn relocate_core(id: &str, rev: u64, entity: &str, to: [i32; 3], facing: Option<([i32; 3], [i32; 3])>, dock_follows: bool) -> Result<yard_core::api::Restructured, StepFailure> {
    let (forward, up) = (facing.map(|f| f.0), facing.map(|f| f.1));
    post_structure(&format!("/api/designs/{id}/relocate-core"), &yard_core::api::RelocateRequest { rev, entity: entity.to_string(), to, forward, up, dock_follows }).await
}

/// Moves the rail a docked entity hangs from, by a cell given in `entity`'s coordinates (see `MoveRailRequest`).
pub async fn move_rail(id: &str, rev: u64, entity: &str, child: &str, to: [i32; 3], carried: bool) -> Result<yard_core::api::Restructured, StepFailure> {
    post_structure(&format!("/api/designs/{id}/move-rail"), &yard_core::api::MoveRailRequest { rev, entity: entity.to_string(), child: child.to_string(), to, carried }).await
}

/// Moves the cores of the sections of a dock chain as an auto-fit found, each with its dock following; all or none.
pub async fn fit_pivots(id: &str, rev: u64, steps: Vec<(String, [i32; 3])>) -> Result<yard_core::api::Restructured, StepFailure> {
    let steps = steps.into_iter().map(|(entity, to)| yard_core::api::PivotStep { entity, to }).collect();
    post_structure(&format!("/api/designs/{id}/fit-pivots"), &yard_core::api::PivotsRequest { rev, steps }).await
}

/// Copies a docked entity in place of another (`replace`) or onto a new rail at `at` in `parent`, facing `facing`.
pub async fn clone_docked(id: &str, rev: u64, source: &str, replace: Option<String>, new: Option<(String, [i32; 3], [i32; 3])>) -> Result<yard_core::api::Restructured, StepFailure> {
    let (parent, at, facing) = match new {
        Some((parent, at, facing)) => (Some(parent), Some(at), Some(facing)),
        None => (None, None, None),
    };
    post_structure(&format!("/api/designs/{id}/clone-docked"), &yard_core::api::CloneRequest { rev, source: source.to_string(), replace, parent, at, facing }).await
}

/// Cuts a box of blocks out of an entity and docks it as a turret.
#[allow(clippy::too_many_arguments)]
pub async fn make_turret(id: &str, rev: u64, entity: &str, min: [i32; 3], max: [i32; 3], base: u8, docker: [i32; 3], exclude: Vec<[i32; 3]>) -> Result<yard_core::api::Restructured, StepFailure> {
    post_structure(&format!("/api/designs/{id}/make-turret"), &yard_core::api::TurretRequest { rev, entity: entity.to_string(), min, max, base, docker: Some(docker), exclude }).await
}

/// Puts the design back as it was before its last relocation or turret.
pub async fn restore_structure(id: &str, rev: u64) -> Result<yard_core::api::Restructured, StepFailure> {
    post_structure(&format!("/api/designs/{id}/restore"), &yard_core::api::RestoreRequest { rev }).await
}
