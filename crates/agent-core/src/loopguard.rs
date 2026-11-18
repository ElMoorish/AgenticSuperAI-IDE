//! Loop detection / circuit breaker (Doc A section D.1): block an identical tool call
//! repeated back to back, so the caller can inject a "reflect or ask the user" message.

use serde_json::Value;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

const WINDOW: usize = 5;

#[derive(Default)]
pub struct LoopGuard {
    recent: VecDeque<u64>,
}

impl LoopGuard {
    /// Returns true if this call should be blocked (same as the immediately previous one).
    /// serde_json maps are key-sorted, so the argument serialization is canonical.
    pub fn should_block(&mut self, name: &str, args: &Value) -> bool {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut h);
        args.to_string().hash(&mut h);
        let id = h.finish();
        let repeat = self.recent.back() == Some(&id);
        if self.recent.len() == WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(id);
        repeat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blocks_immediate_repeat_only() {
        let mut g = LoopGuard::default();
        assert!(!g.should_block("read_file", &json!({"path":"a"})));
        assert!(g.should_block("read_file", &json!({"path":"a"})));
        assert!(!g.should_block("read_file", &json!({"path":"b"})));
        assert!(!g.should_block("read_file", &json!({"path":"a"})));
    }

    #[test]
    fn key_order_does_not_matter() {
        let mut g = LoopGuard::default();
        assert!(!g.should_block("t", &json!({"a":1,"b":2})));
        assert!(g.should_block("t", &json!({"b":2,"a":1})));
    }
}
