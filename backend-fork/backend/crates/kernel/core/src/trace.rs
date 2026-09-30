//! W3C-trace-context-shaped identifiers, threaded through REST → worker →
//! push so a P1 dispatch is one continuous trace and every audit event links
//! back to the request that caused it.

use crate::error::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "TraceContextWire", into = "TraceContextWire")]
pub struct TraceContext {
    trace_id: String,
    span_id: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TraceContextWire {
    trace_id: String,
    span_id: String,
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

impl TraceContext {
    /// 32-hex trace ID + 16-hex span ID (W3C traceparent field shapes).
    pub fn new(
        trace_id: impl Into<String>,
        span_id: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let trace_id: String = trace_id.into();
        let span_id: String = span_id.into();
        if !is_lower_hex(&trace_id, 32) {
            return Err(KernelError::validation(format!(
                "trace_id must be 32 lowercase hex chars, got {trace_id:?}"
            )));
        }
        if !is_lower_hex(&span_id, 16) {
            return Err(KernelError::validation(format!(
                "span_id must be 16 lowercase hex chars, got {span_id:?}"
            )));
        }
        Ok(Self { trace_id, span_id })
    }

    /// Generates a fresh root context (UUID-derived hex).
    #[must_use]
    pub fn generate() -> Self {
        let trace_id = uuid::Uuid::new_v4().simple().to_string();
        let span_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        Self { trace_id, span_id }
    }

    #[must_use]
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    #[must_use]
    pub fn span_id(&self) -> &str {
        &self.span_id
    }
}

impl TryFrom<TraceContextWire> for TraceContext {
    type Error = KernelError;

    fn try_from(value: TraceContextWire) -> Result<Self, Self::Error> {
        Self::new(value.trace_id, value.span_id)
    }
}

impl From<TraceContext> for TraceContextWire {
    fn from(value: TraceContext) -> Self {
        Self {
            trace_id: value.trace_id,
            span_id: value.span_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_context_is_valid_by_construction() {
        let ctx = TraceContext::generate();
        assert_eq!(ctx.trace_id().len(), 32);
        assert_eq!(ctx.span_id().len(), 16);
        let rebuilt = TraceContext::new(ctx.trace_id(), ctx.span_id());
        assert!(rebuilt.is_ok());
    }

    #[test]
    fn rejects_wrong_lengths_and_uppercase() {
        assert!(TraceContext::new("abc", "1234567890abcdef").is_err());
        assert!(TraceContext::new("a".repeat(32), "short").is_err());
        assert!(TraceContext::new("A".repeat(32), "b".repeat(16)).is_err());
    }

    #[test]
    fn serde_roundtrip_validates() {
        let ctx = TraceContext::generate();
        let json = serde_json::to_string(&ctx).unwrap();
        let back: TraceContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, back);
        let bad: Result<TraceContext, _> =
            serde_json::from_str(r#"{"trace_id":"nope","span_id":"nope"}"#);
        assert!(bad.is_err());
    }
}
