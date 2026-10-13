//! Prints every dock record of a blueprint: rail and docker, the docker's
//! turned axes and the motion the shipyard makes of it.
//!
//!     cargo run -q --release -p yard-core --example dockinfo -- <file.sment>
use yard_core::Blueprint;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bp = Blueprint::from_sment(&std::fs::read(&a[0]).unwrap()).unwrap();
    for (path, d) in bp.docks() {
        let axes = yard_core::shape::turned_axes(d.docker.orientation);
        let e = &bp.entity(&path).unwrap().entity;
        let rail_block = bp.entity(path.rsplit_once('/').map_or("", |(p, _)| p)).unwrap().entity.get(d.rail.pos);
        let docker_block = e.get(d.docker.pos);
        println!(
            "{path}: rail ty{} o{} at {:?} (block there ty{} o{}) | docker ty{} o{} at {:?} (block there ty{} o{}) | docker axes {:?} | motion {:?} | blocks {}",
            d.rail.ty, d.rail.orientation, d.rail.pos.to_array(), rail_block.ty(), rail_block.orientation(),
            d.docker.ty, d.docker.orientation, d.docker.pos.to_array(), docker_block.ty(), docker_block.orientation(),
            axes, d.motion(), e.block_count()
        );
    }
}
