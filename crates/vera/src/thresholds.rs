//! Every number the identity and search lanes decide by, with the measurement that chose it.
//!
//! These are properties of the fixtures and (for the embedding ones) of the model they were measured
//! against, not of the algorithms. A port that reproduces the fixture measurements is the same
//! algorithm; one that does not is a bug. A different embedding model re-measures the embedding
//! thresholds before it is trusted (`docs/vera.md` → Proofs).

/// PDQ Hamming distance (of 256 bits) at or under which two frames are the **same picture**: the
/// published PDQ guidance for near-duplicates, and in the VERA studio's measured set every resize
/// and re-export came out far under it while the nearest genuinely different pair sat at 102.
pub const PDQ_TIGHT: u32 = 31;
/// PDQ distance at or under which two frames are **possibly** the same picture (a variant edit, a
/// heavy crop, a burst neighbour): the studio's variant-edit pair sits at 38, inside; the nearest
/// different pair at 102, outside. Nothing automatic acts on this band.
pub const PDQ_LOOSE: u32 = 63;
/// pHash Hamming distance (of 64 bits) for the same picture: the same proportion as PDQ's 31 of
/// 256. Measured: every near-duplicate at 0, the nearest non-duplicate at 10.
pub const PHASH_TIGHT: u32 = 6;
/// pHash distance for a possible match, the same proportion as PDQ's 63 of 256.
pub const PHASH_LOOSE: u32 = 12;
/// PDQ's own 0..100 image-domain quality: how much significant gradient survived the 64×64
/// decimation. Under this a frame is too flat to fingerprint and its digest never makes a tight
/// match on its own (the reference implementation's recommended floor).
pub const PDQ_MIN_QUALITY: u8 = 50;

/// Where a real crop of a frame lands against its source in PDQ bits in the studio's measurement:
/// about 140 of 256, while unrelated pairs sit at 118–120. The number this port's digest
/// implementation is cross-checked against (within [`CROP_PDQ_TOLERANCE`]), and the reason digests
/// alone cannot catch crops: the embedding lane exists for them.
pub const CROP_PDQ_MEASURED: u32 = 140;
/// Bits of slack the cross-check allows between this implementation and the studio's measurement.
pub const CROP_PDQ_TOLERANCE: u32 = 8;

/// Cosine between two frames' image embeddings at or over which they are **duplicates** (the same
/// picture by the model's eye). Measured on `siglip2-so400m-patch14-384`: every true pair clears
/// it, every unrelated pair stays under.
pub const EMBED_DUPLICATE: f32 = 0.82;
/// Cosine at or over which two frames are **near** duplicates, worth showing: where the measured
/// unrelated population (one photographer's work, so already alike) tops out.
pub const EMBED_NEAR: f32 = 0.78;

/// Reciprocal rank fusion's constant, from Cormack, Clarke & Büttcher (2009): large enough that the
/// first few ranks do not dominate, small enough that rank still matters.
pub const RRF_K: f32 = 60.0;

/// Random pairs sampled to estimate a space's background cosine distribution. Enough that the
/// lower half's spread is stable; cheap enough to recompute whenever the space grows.
pub const BACKGROUND_PAIRS: usize = 2000;
/// The lower half of the background pairs is a one-sided sample of the unrelated population; its
/// median absolute deviation times this is that population's standard deviation.
pub const TAIL_MAD_TO_SIGMA: f32 = 2.5;
/// Below this many scored candidates the cutoff does not engage: there is not enough of a list to
/// tell background from signal, and a short list is shown whole.
pub const MIN_SCORED_FOR_CUTOFF: usize = 8;
/// Recompute the background once the space has grown by this fraction since it was last sampled.
pub const BACKGROUND_REFRESH_FRACTION: f32 = 0.10;

/// BM25's term-frequency saturation and length normalisation, the textbook defaults.
pub const BM25_K1: f32 = 1.2;
pub const BM25_B: f32 = 0.75;
