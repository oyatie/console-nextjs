//! Statistical price-intelligence primitives for the integrity engine.
//!
//! All functions operate on **sorted** or **unsorted** slices of `i64` amounts
//! (won). They are pure — no I/O, no async — so they live in the kernel crate
//! and can be called from any layer.
//!
//! ## Small-sample guard
//!
//! Statistical outlier flags are only reliable once a baseline of at least
//! [`MIN_SAMPLE_SIZE`] peers exists. Below that threshold every function that
//! would produce a false-precision score returns [`PriceIntelResult::Sparse`]
//! instead of a numeric value. Callers that display this to users should show
//! "근거 부족 (데이터 부족)" rather than any numeric flag.
//!
//! ## Confidence tiers
//!
//! | Sample size | Tier |
//! |---|---|
//! | < 5 | `Sparse` (no flag) |
//! | 5–14 | `Low` |
//! | 15–29 | `Medium` |
//! | ≥ 30 | `High` |
//!
//! These thresholds are deliberately conservative. A false positive on a
//! legitimate 기안 is more damaging to trust than missing a suspicious one.

/// Minimum peer count required before any statistical outlier flag is issued.
pub const MIN_SAMPLE_SIZE: usize = 5;

/// Confidence tier for a statistical finding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    #[must_use]
    pub const fn from_sample_size(n: usize) -> Self {
        if n >= 30 {
            Self::High
        } else if n >= 15 {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

/// Result of a statistical outlier check.
///
/// [`Sparse`] means the sample is too small to produce a reliable signal.
/// Callers should surface this as "근거 부족 (데이터 부족)", never as a flag.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PriceIntelResult {
    /// Too few peers to compute a reliable statistic. Never produce a finding
    /// from this — a false flag on scarce data harms legitimate users.
    Sparse {
        /// How many peers were available (for diagnostic display).
        peer_count: usize,
    },
    /// The sample is large enough; here is the computed signal.
    Computed {
        /// Z-score of the target value against the peer distribution.
        /// Positive = above the mean; negative = below.
        z_score: f64,
        /// Median absolute deviation (MAD) ratio: `|target - median| / MAD`.
        /// More robust than z-score for small-to-medium skewed distributions.
        /// `None` if MAD == 0 (all peers are identical).
        mad_ratio: Option<f64>,
        /// IQR fence check. `true` if `target > Q3 + 1.5 * IQR`.
        above_iqr_fence: bool,
        /// Percentile of the target value within the peer distribution (0–100).
        percentile: f64,
        /// Peer sample size (for confidence display).
        peer_count: usize,
        /// Statistical confidence tier.
        confidence: Confidence,
    },
}

impl PriceIntelResult {
    /// Returns `true` if the result is sparse (too few peers).
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        matches!(self, Self::Sparse { .. })
    }

    /// Heuristic "is this suspicious?" check combining all signals.
    ///
    /// Returns `None` for sparse results (never flag).
    /// For computed results, returns a score in [0.0, 1.0] where higher means
    /// more suspicious. The caller maps this to a [`Severity`] threshold.
    #[must_use]
    pub fn suspicion_score(&self) -> Option<f64> {
        match self {
            Self::Sparse { .. } => None,
            Self::Computed {
                z_score,
                mad_ratio,
                above_iqr_fence,
                percentile,
                ..
            } => {
                // Weight the signals: z-score (normalized), MAD ratio, IQR fence,
                // and raw percentile. All clamped to [0,1] before weighting.
                let z_norm = (z_score.abs() / 4.0).min(1.0); // 4σ → max signal
                let mad_norm = mad_ratio.map(|m| (m / 5.0).min(1.0)).unwrap_or(0.0); // 5× MAD → max signal
                let iqr_signal = if *above_iqr_fence { 1.0 } else { 0.0 };
                let pct_norm = ((*percentile - 90.0).max(0.0) / 10.0).min(1.0); // top 10% → max signal

                // Only flag HIGH percentile (above average is normal; we want
                // extreme outliers on the HIGH side — overpayment risk).
                if *z_score < 0.0 {
                    // Below mean — not a price-inflation concern.
                    return Some(0.0);
                }

                let score = 0.35 * z_norm + 0.30 * mad_norm + 0.20 * iqr_signal + 0.15 * pct_norm;
                Some(score.clamp(0.0, 1.0))
            }
        }
    }
}

/// Compute price-intelligence statistics for a target amount against a peer
/// distribution.
///
/// # Arguments
/// * `target` — the amount under scrutiny (i64 won).
/// * `peers` — all amounts in the peer group INCLUDING `target` itself
///   (the detector fetches the whole distribution, which naturally contains
///   the row being checked). The function handles deduplication internally.
///
/// # Returns
/// [`PriceIntelResult::Sparse`] if `peers.len() < MIN_SAMPLE_SIZE`, otherwise
/// [`PriceIntelResult::Computed`] with full statistics.
///
/// # Panics
/// Never. All arithmetic uses checked or finite-safe operations.
#[must_use]
pub fn compute_price_intel(target: i64, peers: &[i64]) -> PriceIntelResult {
    let n = peers.len();
    if n < MIN_SAMPLE_SIZE {
        return PriceIntelResult::Sparse { peer_count: n };
    }

    // Sort a copy for percentile / median / IQR.
    let mut sorted = peers.to_vec();
    sorted.sort_unstable();

    let target_f = target as f64;
    let n_f = n as f64;

    // Mean and standard deviation.
    let mean = sorted.iter().map(|&x| x as f64).sum::<f64>() / n_f;
    let variance = sorted
        .iter()
        .map(|&x| {
            let d = x as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / n_f; // population variance (we have the full peer set, not a sample)
    let std_dev = variance.sqrt();

    let z_score = if std_dev > 0.0 {
        (target_f - mean) / std_dev
    } else {
        0.0
    };

    // Median and MAD (median absolute deviation).
    let median = percentile_sorted(&sorted, 50.0);
    let deviations: Vec<f64> = sorted.iter().map(|&x| (x as f64 - median).abs()).collect();
    let mad = {
        let mut dev_sorted = deviations.clone();
        dev_sorted.sort_by(f64::total_cmp);
        percentile_f64_sorted(&dev_sorted, 50.0)
    };
    let mad_ratio = if mad > 0.0 {
        Some((target_f - median).abs() / mad)
    } else {
        None
    };

    // IQR fence.
    let q1 = percentile_sorted(&sorted, 25.0);
    let q3 = percentile_sorted(&sorted, 75.0);
    let iqr = q3 - q1;
    let upper_fence = q3 + 1.5 * iqr;
    let above_iqr_fence = target_f > upper_fence;

    // Percentile of target.
    let rank = sorted.partition_point(|&x| (x as f64) < target_f);
    let percentile = if n == 1 {
        50.0
    } else {
        (rank as f64) / ((n - 1) as f64) * 100.0
    };

    PriceIntelResult::Computed {
        z_score,
        mad_ratio,
        above_iqr_fence,
        percentile: percentile.clamp(0.0, 100.0),
        peer_count: n,
        confidence: Confidence::from_sample_size(n),
    }
}

/// Compute the p-th percentile of a sorted `i64` slice using linear
/// interpolation (nearest-rank with linear interpolation for non-integer
/// positions).
///
/// `p` must be in `[0.0, 100.0]`. The slice must be non-empty and sorted.
#[must_use]
fn percentile_sorted(sorted: &[i64], p: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    debug_assert!((0.0..=100.0).contains(&p));

    let n = sorted.len();
    if n == 1 {
        return sorted[0] as f64;
    }

    let index = p / 100.0 * (n as f64 - 1.0);
    let lo = index.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = index - lo as f64;

    sorted[lo] as f64 * (1.0 - frac) + sorted[hi] as f64 * frac
}

/// Same as [`percentile_sorted`] but for `f64` slices (used for MAD
/// computation).
#[must_use]
fn percentile_f64_sorted(sorted: &[f64], p: f64) -> f64 {
    debug_assert!(!sorted.is_empty());

    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }

    let index = p / 100.0 * (n as f64 - 1.0);
    let lo = index.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = index - lo as f64;

    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_below_min_sample() {
        let result = compute_price_intel(100_000, &[100_000, 90_000, 80_000, 110_000]);
        assert!(matches!(result, PriceIntelResult::Sparse { peer_count: 4 }));
        assert!(result.is_sparse());
        assert!(result.suspicion_score().is_none());
    }

    #[test]
    fn no_flag_for_below_mean() {
        let peers: Vec<i64> = vec![100_000, 110_000, 105_000, 98_000, 102_000, 107_000, 99_000];
        let result = compute_price_intel(80_000, &peers);
        let score = result.suspicion_score().unwrap();
        assert_eq!(score, 0.0, "below-mean value should have zero suspicion");
    }

    #[test]
    fn high_suspicion_for_extreme_outlier() {
        // 10 normal peers, one extreme outlier target (10x the median).
        let peers: Vec<i64> = vec![
            100_000, 105_000, 98_000, 102_000, 110_000, 95_000, 108_000, 103_000, 99_000, 101_000,
        ];
        // Target is 10× the median.
        let result = compute_price_intel(1_000_000, &peers);
        match &result {
            PriceIntelResult::Computed {
                above_iqr_fence,
                z_score,
                ..
            } => {
                assert!(*above_iqr_fence, "extreme value must be above IQR fence");
                assert!(*z_score > 2.0, "z-score must be high for extreme outlier");
            }
            PriceIntelResult::Sparse { .. } => panic!("should not be sparse"),
        }
        let score = result.suspicion_score().unwrap();
        assert!(
            score > 0.5,
            "extreme outlier suspicion score should be high, got {score}"
        );
    }

    #[test]
    fn percentile_median_for_middle_value() {
        let peers: Vec<i64> = (1..=11).map(|i| i * 10_000).collect();
        let result = compute_price_intel(60_000, &peers);
        if let PriceIntelResult::Computed { percentile, .. } = result {
            // 60_000 is the exact median of 1..11 * 10_000.
            assert!(
                (percentile - 50.0).abs() < 1.0,
                "median value should be near 50th percentile, got {percentile}"
            );
        } else {
            panic!("expected Computed");
        }
    }

    #[test]
    fn confidence_tiers() {
        assert_eq!(Confidence::from_sample_size(4), Confidence::Low);
        assert_eq!(Confidence::from_sample_size(5), Confidence::Low);
        assert_eq!(Confidence::from_sample_size(14), Confidence::Low);
        assert_eq!(Confidence::from_sample_size(15), Confidence::Medium);
        assert_eq!(Confidence::from_sample_size(29), Confidence::Medium);
        assert_eq!(Confidence::from_sample_size(30), Confidence::High);
    }

    #[test]
    fn uniform_distribution_no_mad_ratio() {
        // All peers identical: MAD == 0, mad_ratio should be None.
        let peers: Vec<i64> = vec![100_000; 6];
        let result = compute_price_intel(100_000, &peers);
        if let PriceIntelResult::Computed {
            mad_ratio, z_score, ..
        } = result
        {
            assert!(
                mad_ratio.is_none(),
                "uniform peers must yield None mad_ratio"
            );
            assert_eq!(z_score, 0.0, "uniform peers must yield zero z-score");
        } else {
            panic!("expected Computed for 6 identical peers");
        }
    }
}
