//! glTF 2.0 import: bake every triangle in a file into one [`Mesh`].
//!
//! The venue wants a single indexed mesh in metres, not a scene graph, so
//! this walks the document's scenes and nodes (applying each node's world
//! transform), reads every primitive's `POSITION` and optional `NORMAL`,
//! `TEXCOORD_0` and indices, and concatenates the result. The `gltf` crate
//! (Apache-2.0 OR MIT) handles buffers, accessors and base64 data URIs.

use glam::{Mat3, Mat4, Vec3};
use gltf::mesh::Mode;

use crate::Mesh;

#[derive(Debug)]
pub enum GltfError {
    Io(std::io::Error),
    Gltf(gltf::Error),
    Empty,
}

impl std::fmt::Display for GltfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GltfError::Io(e) => write!(f, "cannot read glTF: {e}"),
            GltfError::Gltf(e) => write!(f, "cannot parse glTF: {e}"),
            GltfError::Empty => write!(f, "glTF contains no triangles"),
        }
    }
}

impl std::error::Error for GltfError {}

impl From<std::io::Error> for GltfError {
    fn from(e: std::io::Error) -> Self {
        GltfError::Io(e)
    }
}

impl From<gltf::Error> for GltfError {
    fn from(e: gltf::Error) -> Self {
        GltfError::Gltf(e)
    }
}

/// Load `path` and flatten it to one mesh in metres.
pub fn load_gltf(path: &std::path::Path) -> Result<Mesh, GltfError> {
    let (document, buffers, _images) = gltf::import(path)?;
    let mut out = Mesh::new();

    let roots = document
        .default_scene()
        .map(|scene| scene.nodes().collect::<Vec<_>>())
        .unwrap_or_else(|| document.scenes().flat_map(|s| s.nodes()).collect());

    for node in roots {
        walk(node, Mat4::IDENTITY, &buffers, &mut out);
    }

    if out.indices.is_empty() {
        return Err(GltfError::Empty);
    }
    Ok(out)
}

fn walk(node: gltf::Node<'_>, parent: Mat4, buffers: &[gltf::buffer::Data], out: &mut Mesh) {
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;

    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            if primitive.mode() != Mode::Triangles {
                continue;
            }
            read_primitive(&primitive, world, buffers, out);
        }
    }

    for child in node.children() {
        walk(child, world, buffers, out);
    }
}

fn read_primitive(
    primitive: &gltf::Primitive<'_>,
    world: Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut Mesh,
) {
    let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|b| &b.0[..]));
    let Some(positions) = reader.read_positions() else {
        return;
    };

    let base = out.positions.len() as u32;
    let normal_mat = {
        let a = Mat3::from_mat4(world);
        if a.determinant().abs() > 1.0e-12 {
            a.inverse().transpose()
        } else {
            a
        }
    };
    let transform = |p: [f32; 3]| {
        let v = world * glam::Vec4::new(p[0], p[1], p[2], 1.0);
        [v.x, v.y, v.z]
    };
    let rotate = |n: [f32; 3]| {
        let v = (normal_mat * Vec3::from_array(n)).normalize_or_zero();
        [v.x, v.y, v.z]
    };

    for p in positions {
        out.positions.push(transform(p));
    }
    if let Some(normals) = reader.read_normals() {
        for n in normals {
            out.normals.push(rotate(n));
        }
    } else {
        out.normals.resize(out.positions.len(), [0.0, 1.0, 0.0]);
    }
    if let Some(uvs) = reader.read_tex_coords(0) {
        for uv in uvs.into_f32() {
            out.uvs.push(uv);
        }
    } else {
        out.uvs.resize(out.positions.len(), [0.0, 0.0]);
    }

    let vertex_count = out.positions.len() as u32 - base;
    match reader.read_indices() {
        Some(indices) => {
            for i in indices.into_u32() {
                out.indices.push(base + i);
            }
        }
        None => {
            for i in 0..vertex_count {
                out.indices.push(base + i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("metal-geom-gltf-{}-{name}", std::process::id()))
    }

    /// A minimal glTF with one embedded triangle, as a JSON `.gltf` with the
    /// buffer inline as a base64 data URI.
    #[test]
    fn loads_one_triangle_from_a_data_uri() {
        // Three f32 positions (0,0,0), (1,0,0), (0,1,0) = 20*3 = 60 bytes.
        let mut bytes = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in p {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
        }
        for i in [0u32, 1, 2] {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        let b64 = base64(&bytes);
        let json = format!(
            r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0,
  "scenes": [{{ "nodes": [0] }}],
  "nodes": [{{ "mesh": 0 }}],
  "meshes": [{{ "primitives": [{{ "attributes": {{ "POSITION": 0 }}, "indices": 1 }}] }}],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
       "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR" }}
  ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": 36, "byteLength": 12 }}
  ],
  "buffers": [{{
    "byteLength": 48,
    "uri": "data:application/octet-stream;base64,{b64}"
  }}]
}}"#
        );
        let path = temp_path("triangle.gltf");
        std::fs::write(&path, json).unwrap();
        let result = load_gltf(&path);
        let _ = std::fs::remove_file(&path);

        let mesh = result.unwrap();
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(mesh.positions[2], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn empty_file_is_empty_not_a_mesh() {
        let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[]}],"nodes":[]}"#;
        let path = temp_path("empty.gltf");
        std::fs::write(&path, json).unwrap();
        let result = load_gltf(&path);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(result, Err(GltfError::Empty)));
    }

    fn base64(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }
}
