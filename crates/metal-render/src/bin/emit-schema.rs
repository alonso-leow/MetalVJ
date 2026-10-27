//! Rewrites `data/schema.json` from the renderers themselves.
//!
//!     cargo run -p metal-render --bin emit-schema
//!
//! Run it after adding a model, adding a float to one, or reordering a
//! `Schema`'s blocks. `tests/schema.rs` fails if you forget.

fn main() {
    let path = metal_render::manifest::schema_path();
    let json = metal_render::manifest::manifest_json();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create data/");
    }
    std::fs::write(&path, &json).expect("write schema.json");
    println!("wrote {} ({} bytes)", path.display(), json.len());
}
