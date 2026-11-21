//! Writes the editor view of a project as JSON, for the UI's browser mock:
//! `cargo run -p plyroll-core --example dump_view -- examples/demo/demo.plyroll > app/src/mock/demo-view.json`

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: dump_view <project.plyroll>"))?;
    let mut proj = plyroll_core::project::Project::load(std::path::Path::new(&path))?;
    // Leave a few plies unset / flagged so the mock shows every state.
    if let Some(p) = proj.plies.iter_mut().find(|p| p.part == "rib") {
        p.reference = None;
        p.confirmed = false;
    }
    if let Some(p) = proj.plies.iter_mut().find(|p| p.part == "skin") {
        p.confirmed = false;
        p.note = Some("outline changed in the DXF; check the 0° reference".into());
    }
    let plies = plyroll_core::view::project_view(&proj);
    println!("{}", serde_json::to_string(&plies)?);
    Ok(())
}
