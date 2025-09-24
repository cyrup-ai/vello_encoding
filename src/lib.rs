// Copyright 2023 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Raw scene encoding.

// LINEBENDER LINT SET - lib.rs - v2
// See https://linebender.org/wiki/canonical-lints/
// These lints aren't included in Cargo.toml because they
// shouldn't apply to examples and tests
#![warn(unused_crate_dependencies)]
#![warn(clippy::print_stdout, clippy::print_stderr)]
// Targeting e.g. 32-bit means structs containing usize can give false positives for 64-bit.
#![cfg_attr(target_pointer_width = "64", warn(clippy::trivially_copy_pass_by_ref))]
// END LINEBENDER LINT SET
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
// The following lints are part of the Linebender standard set,
// but resolving them has been deferred for now.
// Feel free to send a PR that solves one or more of these.
// Need to allow instead of expect until Rust 1.83 https://github.com/rust-lang/rust/pull/130025
#![allow(missing_docs, reason = "We have many as-yet undocumented items.")]
#![expect(
    missing_debug_implementations,
    clippy::cast_possible_truncation,
    clippy::missing_assert_message,
    reason = "Deferred"
)]
#![allow(
    unnameable_types,
    clippy::shadow_unrelated,
    reason = "Deferred, only apply in some feature sets so not expect"
)]

mod binning;
mod clip;
mod config;
mod draw;
mod encoding;
#[cfg(feature = "bump_estimate")]
mod estimate;
mod glyph;
mod glyph_cache;
mod image_cache;
mod mask;
pub mod math;
mod monoid;
mod path;
mod ramp_cache;
mod resolve;

pub use binning::BinHeader;
pub use clip::{Clip, ClipBbox, ClipBic, ClipElement};
pub use config::{
    BufferSize, BufferSizes, BumpAllocatorMemory, BumpAllocators, ConfigUniform, IndirectCount,
    RenderConfig, WorkgroupCounts, WorkgroupSize,
};
pub use draw::{
    DRAW_INFO_FLAGS_FILL_RULE_BIT, DrawBbox, DrawBeginClip, DrawBlurRoundedRect, DrawColor,
    DrawImage, DrawLinearGradient, DrawMonoid, DrawRadialGradient, DrawSweepGradient, DrawTag,
};
pub use encoding::{Encoding, Resources, StreamOffsets};
#[cfg(feature = "bump_estimate")]
pub use estimate::BumpEstimator;
pub use glyph::{Glyph, GlyphRun};
pub use mask::{make_mask_lut, make_mask_lut_16};
pub use math::Transform;
pub use monoid::Monoid;
pub use path::{
    Cubic, LineSoup, Path, PathBbox, PathEncoder, PathMonoid, PathSegment, PathSegmentType,
    PathTag, SegmentCount, Style, Tile,
};
pub use ramp_cache::Ramps;
pub use resolve::{Layout, Patch, Resolver, resolve_solid_paths_only};

/// A normalized variation coordinate (for variable fonts) in 2.14 fixed point format.
///
/// In most cases, this can be [cast](bytemuck::cast_slice) from the
/// normalised coords provided by your text layout library.
///
/// Normalized coordinate for variable fonts, representing design space coordinates
/// normalized to the range [-1.0, 1.0] and quantized to 16-bit signed integers.
///
/// This type is compatible with glyphon and other text rendering libraries.
pub type NormalizedCoord = i16;
