//! I.4: every operation of the closed vocabulary really compiles and
//! rasterises — it is not enough for the WGSL to look right, it has to go
//! through `naga` and a real GPU, like any shader the AI would write. And
//! this crate's vocabulary has to be exactly the one
//! `metal_schema::ir::KNOWN_OPS` declares closed — see `metal_forge::vocab`'s
//! doc.

use metal_forge::vocab::VOCABULARY;
use metal_forge::{compile, raster};

#[test]
fn every_vocabulary_op_compiles() {
    for op in VOCABULARY {
        compile(op.wgsl).unwrap_or_else(|e| panic!("vocab op {:?} failed to compile: {e}", op.name));
    }
}

/// I.4's "three moments" applied to the vocabulary itself: every operation
/// has to rasterise at the minimum, default and maximum of its own float
/// range [0, 1] — not just compile.
#[test]
fn every_vocabulary_op_rasters_at_min_default_and_max() {
    for op in VOCABULARY {
        let pipeline = compile(op.wgsl).unwrap_or_else(|e| panic!("vocab op {:?} failed to compile: {e}", op.name));
        for (moment, floats) in [("min", [0.0, 0.0, 0.0, 0.0]), ("default", [0.5, 0.5, 0.5, 0.5]), ("max", [1.0, 1.0, 1.0, 1.0])] {
            raster(&pipeline, &floats, 16, 16)
                .unwrap_or_else(|e| panic!("vocab op {:?} failed to raster at {moment}: {e}", op.name));
        }
    }
}

#[test]
fn vocabulary_names_are_exactly_the_closed_ir_vocabulary() {
    let mut vocab_names: Vec<&str> = VOCABULARY.iter().map(|o| o.name).collect();
    let mut known_ops: Vec<&str> = metal_schema::ir::KNOWN_OPS.to_vec();
    vocab_names.sort_unstable();
    known_ops.sort_unstable();
    assert_eq!(
        vocab_names, known_ops,
        "metal_forge::vocab::VOCABULARY and metal_schema::ir::KNOWN_OPS fell out of sync"
    );
}

#[test]
fn every_vocabulary_op_declares_at_least_one_float_name() {
    for op in VOCABULARY {
        assert!(!op.float_names.is_empty(), "vocab op {:?} documents no float", op.name);
    }
}

#[test]
fn find_looks_up_by_name_and_rejects_unknown_ones() {
    assert!(metal_forge::vocab::find("warp").is_some());
    assert!(metal_forge::vocab::find("invent_a_shader").is_none());
}
