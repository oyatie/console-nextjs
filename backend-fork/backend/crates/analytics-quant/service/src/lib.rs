//! Deterministic statistical projection service.
//!
//! Given a historical value series and a horizon, produces a forward point
//! estimate, a 95% confidence band, and a fat-tail CVaR (expected shortfall)
//! at the 5% level. The model assumes EWMA volatility and Student-t(ν=4)
//! innovations, propagated through a **seeded** Monte-Carlo so the same input
//! always yields the same output (required for reproducible tests and the
//! platform's §4-20 determinism guarantee). The lower-tail CVaR is refined
//! with a peaks-over-threshold EVT (Generalized-Pareto) fit.
//!
//! This is a pure, read-only compute crate: no I/O, no PII, no persistence,
//! and therefore no tenant table and no RLS surface.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use serde::{Deserialize, Serialize};

/// RiskMetrics EWMA decay for the volatility recursion.
const EWMA_LAMBDA: f64 = 0.94;
/// Student-t degrees of freedom for the innovation distribution (fat tails).
const STUDENT_T_NU: f64 = 4.0;
/// Monte-Carlo path count. Fixed so results are reproducible.
const SIMULATIONS: u32 = 10_000;
/// Fixed RNG seed — reproducibility over unpredictability (this is not crypto).
const SEED: u64 = 0x5EED_A11A_5EED_A11A;
/// Minimum observations required to estimate a drift + volatility.
const MIN_SERIES_LEN: usize = 3;
/// Hard ceiling on the horizon so a request cannot force an unbounded sim.
const MAX_HORIZON: u32 = 3650;
/// Tail-probability boundary for VaR/CVaR (worst 5%).
const TAIL_Q: f64 = 0.05;
/// Minimum tail exceedances before an EVT fit is trusted over the empirical mean.
const MIN_TAIL_POINTS: usize = 20;

/// How the series values compose over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeriesKind {
    /// Multiplicative / price-like: returns are ratios, value floored at 0.
    Money,
    /// Additive / rate-like: returns are arithmetic differences.
    Percent,
}

/// A projection request.
#[derive(Debug, Clone)]
pub struct ProjectionRequest {
    /// Ordered historical values, oldest first.
    pub series: Vec<f64>,
    /// Number of forward steps to project.
    pub horizon: u32,
    /// Composition rule.
    pub kind: SeriesKind,
}

/// Model assumptions echoed back for auditability.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Assumptions {
    /// Final EWMA volatility σ used for the innovations.
    pub ewma_volatility: f64,
    /// Student-t degrees of freedom.
    pub student_t_nu: f64,
    /// Estimated per-step drift μ.
    pub drift: f64,
    /// Monte-Carlo path count.
    pub simulations: u32,
    /// RNG seed (echoed to prove determinism).
    pub seed: u64,
}

/// A projection result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ProjectionResult {
    /// Deterministic central projection.
    pub point_estimate: f64,
    /// Lower bound of the 95% band (2.5th percentile of terminal outcomes).
    pub ci95_low: f64,
    /// Upper bound of the 95% band (97.5th percentile).
    pub ci95_high: f64,
    /// Expected shortfall in the worst 5% of outcomes (EVT-refined, fat tail).
    pub cvar95: f64,
    /// Echoed assumptions.
    pub assumptions: Assumptions,
}

/// Reasons a projection cannot be computed. All are caller errors (fail-closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuantError {
    /// The series was empty.
    #[error("series is empty")]
    EmptySeries,
    /// Fewer than [`MIN_SERIES_LEN`] observations.
    #[error("series needs at least {MIN_SERIES_LEN} observations")]
    TooShort,
    /// A money series contained a non-positive value (ratios undefined).
    #[error("money series values must be strictly positive")]
    NonPositiveValue,
    /// The series contained a non-finite value.
    #[error("series values must be finite")]
    NonFiniteValue,
    /// Horizon was zero or above [`MAX_HORIZON`].
    #[error("horizon must be between 1 and {MAX_HORIZON}")]
    InvalidHorizon,
}

/// A tiny seeded splitmix64 PRNG. Deterministic, no dependency, adequate for
/// Monte-Carlo (not cryptographic — reproducibility is the whole point).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in the open interval (0, 1), 53-bit resolution. The +0.5 offset
    /// keeps it strictly off both endpoints so the t-quantile never diverges.
    fn next_unit(&mut self) -> f64 {
        let bits = self.next_u64() >> 11; // top 53 bits
        (bits as f64 + 0.5) / 9_007_199_254_740_992.0 // 2^53
    }
}

/// Inverse CDF (quantile) of Student-t with ν=4, closed form.
///
/// For p in (0,1): with a = 4·p·(1−p), q = cos(acos(√a)/3)/√a,
/// t = sign(p−½)·2·√(q−1). Exact for four degrees of freedom.
fn student_t4_quantile(p: f64) -> f64 {
    let a = 4.0 * p * (1.0 - p);
    let sqrt_a = a.sqrt();
    let q = (sqrt_a.acos() / 3.0).cos() / sqrt_a;
    let t = 2.0 * (q - 1.0).max(0.0).sqrt();
    if p < 0.5 { -t } else { t }
}

/// A standardized (unit-variance) fat-tailed innovation. Student-t(ν=4) has
/// variance ν/(ν−2)=2, so dividing by √2 rescales it to unit variance.
fn standardized_innovation(rng: &mut SplitMix64) -> f64 {
    let t_variance = STUDENT_T_NU / (STUDENT_T_NU - 2.0); // = 2 for ν=4
    student_t4_quantile(rng.next_unit()) / t_variance.sqrt()
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Population variance.
fn variance(xs: &[f64], m: f64) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / xs.len() as f64
}

/// Per-step returns from the series, per composition rule. Validates values.
fn returns(series: &[f64], kind: SeriesKind) -> Result<Vec<f64>, QuantError> {
    let mut out = Vec::with_capacity(series.len().saturating_sub(1));
    for pair in series.windows(2) {
        let (prev, cur) = (pair[0], pair[1]);
        if !prev.is_finite() || !cur.is_finite() {
            return Err(QuantError::NonFiniteValue);
        }
        match kind {
            SeriesKind::Money => {
                if prev <= 0.0 || cur <= 0.0 {
                    return Err(QuantError::NonPositiveValue);
                }
                out.push(cur / prev - 1.0);
            }
            SeriesKind::Percent => out.push(cur - prev),
        }
    }
    Ok(out)
}

/// Final EWMA volatility σ. Seeded with the sample variance, then decayed over
/// the squared deviations from the drift.
fn ewma_volatility(rets: &[f64], drift: f64) -> f64 {
    let mut var = variance(rets, drift);
    for r in rets {
        let dev = r - drift;
        var = EWMA_LAMBDA * var + (1.0 - EWMA_LAMBDA) * dev * dev;
    }
    var.max(0.0).sqrt()
}

/// Linear-interpolated empirical quantile of an ascending-sorted slice.
fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    match sorted.len() {
        0 => f64::NAN,
        1 => sorted[0],
        n => {
            let pos = q.clamp(0.0, 1.0) * (n - 1) as f64;
            let lo = pos.floor() as usize;
            let hi = pos.ceil() as usize;
            let frac = pos - lo as f64;
            sorted[lo] + (sorted[hi] - sorted[lo]) * frac
        }
    }
}

/// CVaR (expected shortfall) of the worst-`TAIL_Q` outcomes, refined with a
/// Generalized-Pareto peaks-over-threshold fit for the fat lower tail.
///
/// `threshold` is the outcome at the tail boundary (the 5th percentile). We fit
/// a GPD to the exceedances below it via method-of-moments and take the mean
/// excess `β/(1−ξ)` as the tail deepening. Falls back to the empirical tail
/// mean when there are too few points or the fit is degenerate. The result is
/// always ≤ `threshold`, so the "CVaR95 ≤ P5" invariant holds by construction.
fn tail_cvar(sorted_ascending: &[f64], threshold: f64) -> f64 {
    let tail: Vec<f64> = sorted_ascending
        .iter()
        .copied()
        .filter(|&x| x <= threshold)
        .collect();
    if tail.is_empty() {
        return threshold;
    }
    let empirical = mean(&tail);

    // Exceedances (positive losses beyond the threshold).
    let exceed: Vec<f64> = tail.iter().map(|&x| threshold - x).collect();
    if exceed.len() < MIN_TAIL_POINTS {
        return empirical;
    }
    let m = mean(&exceed);
    let s2 = variance(&exceed, m);
    if m <= 0.0 || s2 <= 0.0 {
        return empirical;
    }
    // GPD method-of-moments: ratio = m²/s².
    let ratio = m * m / s2;
    let xi = 0.5 * (1.0 - ratio);
    let beta = 0.5 * m * (ratio + 1.0);
    if xi >= 1.0 || beta <= 0.0 {
        // Infinite/undefined mean-excess: fall back to the empirical estimate.
        return empirical;
    }
    let mean_excess = beta / (1.0 - xi);
    // Deepen the tail but never rise above the threshold.
    (threshold - mean_excess).min(threshold)
}

/// Compute the projection. Fail-closed on any malformed input.
pub fn project(req: &ProjectionRequest) -> Result<ProjectionResult, QuantError> {
    if req.series.is_empty() {
        return Err(QuantError::EmptySeries);
    }
    if req.series.len() < MIN_SERIES_LEN {
        return Err(QuantError::TooShort);
    }
    if req.horizon == 0 || req.horizon > MAX_HORIZON {
        return Err(QuantError::InvalidHorizon);
    }
    let last = req.series[req.series.len() - 1];
    if !last.is_finite() {
        return Err(QuantError::NonFiniteValue);
    }

    let rets = returns(&req.series, req.kind)?;
    let drift = mean(&rets);
    let sigma = ewma_volatility(&rets, drift);

    let horizon = req.horizon;
    let point_estimate = match req.kind {
        SeriesKind::Money => last * (1.0 + drift).powi(horizon as i32),
        SeriesKind::Percent => last + drift * f64::from(horizon),
    };

    // Seeded Monte-Carlo over `SIMULATIONS` paths of `horizon` steps.
    let mut rng = SplitMix64::new(SEED);
    let mut terminals = Vec::with_capacity(SIMULATIONS as usize);
    for _ in 0..SIMULATIONS {
        let mut value = last;
        for _ in 0..horizon {
            let shock = drift + sigma * standardized_innovation(&mut rng);
            match req.kind {
                SeriesKind::Money => value *= (1.0 + shock).max(0.0),
                SeriesKind::Percent => value += shock,
            }
        }
        terminals.push(value);
    }
    terminals.sort_by(f64::total_cmp);

    let ci95_low = quantile_sorted(&terminals, 0.025);
    let ci95_high = quantile_sorted(&terminals, 0.975);
    let p5 = quantile_sorted(&terminals, TAIL_Q);
    let cvar95 = tail_cvar(&terminals, p5);

    Ok(ProjectionResult {
        point_estimate,
        ci95_low,
        ci95_high,
        cvar95,
        assumptions: Assumptions {
            ewma_volatility: sigma,
            student_t_nu: STUDENT_T_NU,
            drift,
            simulations: SIMULATIONS,
            seed: SEED,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(series: &[f64], horizon: u32, kind: SeriesKind) -> ProjectionRequest {
        ProjectionRequest {
            series: series.to_vec(),
            horizon,
            kind,
        }
    }

    #[test]
    fn student_t4_quantile_is_symmetric_and_centered() {
        assert!((student_t4_quantile(0.5)).abs() < 1e-12);
        let lo = student_t4_quantile(0.25);
        let hi = student_t4_quantile(0.75);
        assert!((lo + hi).abs() < 1e-9, "t4 quantile must be symmetric");
        // Known value: t_4(0.975) ≈ 2.7764.
        assert!((student_t4_quantile(0.975) - 2.7764).abs() < 1e-3);
    }

    #[test]
    fn rejects_bad_input_fail_closed() {
        assert_eq!(
            project(&req(&[], 5, SeriesKind::Money)),
            Err(QuantError::EmptySeries)
        );
        assert_eq!(
            project(&req(&[1.0, 2.0], 5, SeriesKind::Money)),
            Err(QuantError::TooShort)
        );
        assert_eq!(
            project(&req(&[1.0, 2.0, 3.0], 0, SeriesKind::Money)),
            Err(QuantError::InvalidHorizon)
        );
        assert_eq!(
            project(&req(&[1.0, 2.0, 3.0], MAX_HORIZON + 1, SeriesKind::Money)),
            Err(QuantError::InvalidHorizon)
        );
        assert_eq!(
            project(&req(&[100.0, 0.0, 121.0], 5, SeriesKind::Money)),
            Err(QuantError::NonPositiveValue)
        );
        assert_eq!(
            project(&req(&[1.0, f64::NAN, 3.0], 5, SeriesKind::Percent)),
            Err(QuantError::NonFiniteValue)
        );
    }

    #[test]
    fn zero_volatility_money_is_exact() {
        // Constant 10% growth => drift 0.1, σ = 0. No MC dispersion.
        let out = project(&req(&[100.0, 110.0, 121.0, 133.1], 2, SeriesKind::Money)).unwrap();
        let expected = 133.1 * 1.1_f64 * 1.1; // (1+0.1)^2
        assert!((out.point_estimate - expected).abs() < 1e-6);
        assert!(out.assumptions.ewma_volatility.abs() < 1e-12);
        // Band collapses onto the point estimate; CVaR equals it.
        assert!((out.ci95_low - expected).abs() < 1e-6);
        assert!((out.ci95_high - expected).abs() < 1e-6);
        assert!((out.cvar95 - expected).abs() < 1e-6);
        assert!((out.assumptions.drift - 0.1).abs() < 1e-12);
    }

    #[test]
    fn zero_volatility_percent_is_exact() {
        // Constant +1.0 step => drift 1.0, σ = 0.
        let out = project(&req(&[1.0, 2.0, 3.0, 4.0], 3, SeriesKind::Percent)).unwrap();
        assert!((out.point_estimate - 7.0).abs() < 1e-9); // 4 + 1*3
        assert!(out.assumptions.ewma_volatility.abs() < 1e-12);
        assert!((out.ci95_low - 7.0).abs() < 1e-9);
        assert!((out.ci95_high - 7.0).abs() < 1e-9);
        assert!((out.cvar95 - 7.0).abs() < 1e-9);
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let r = req(
            &[100.0, 103.0, 99.0, 105.0, 101.0, 108.0],
            10,
            SeriesKind::Money,
        );
        let a = project(&r).unwrap();
        let b = project(&r).unwrap();
        assert_eq!(a, b, "seeded projection must be bit-reproducible");
    }

    #[test]
    fn ci_contains_point_and_cvar_below_p5() {
        let out = project(&req(
            &[100.0, 103.0, 99.0, 105.0, 101.0, 108.0, 104.0, 110.0],
            20,
            SeriesKind::Money,
        ))
        .unwrap();
        assert!(out.ci95_low <= out.ci95_high);
        assert!(
            out.ci95_low <= out.point_estimate && out.point_estimate <= out.ci95_high,
            "CI95 [{}, {}] must contain point {}",
            out.ci95_low,
            out.ci95_high,
            out.point_estimate
        );
        let p5 = quantile_sorted(
            &{
                // Recompute P5 the same way project does, for the invariant check.
                let mut r = project_terminals(&req(
                    &[100.0, 103.0, 99.0, 105.0, 101.0, 108.0, 104.0, 110.0],
                    20,
                    SeriesKind::Money,
                ));
                r.sort_by(f64::total_cmp);
                r
            },
            TAIL_Q,
        );
        assert!(
            out.cvar95 <= p5 + 1e-9,
            "CVaR95 {} must be <= P5 {}",
            out.cvar95,
            p5
        );
        assert!(out.assumptions.ewma_volatility > 0.0);
    }

    // Test-only helper mirroring the MC terminal generation for invariant checks.
    fn project_terminals(req: &ProjectionRequest) -> Vec<f64> {
        let rets = returns(&req.series, req.kind).unwrap();
        let drift = mean(&rets);
        let sigma = ewma_volatility(&rets, drift);
        let last = req.series[req.series.len() - 1];
        let mut rng = SplitMix64::new(SEED);
        let mut terminals = Vec::with_capacity(SIMULATIONS as usize);
        for _ in 0..SIMULATIONS {
            let mut value = last;
            for _ in 0..req.horizon {
                let shock = drift + sigma * standardized_innovation(&mut rng);
                match req.kind {
                    SeriesKind::Money => value *= (1.0 + shock).max(0.0),
                    SeriesKind::Percent => value += shock,
                }
            }
            terminals.push(value);
        }
        terminals
    }
}
