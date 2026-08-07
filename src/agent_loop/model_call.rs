use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::context::PromptContext;
use crate::event::AgentEvent;
use crate::provider::{FailedTurn, Provider, ProviderEvent, StreamTerminal, TurnOutput};
use crate::tools::ToolSpec;

/// Request-level retry policy. Replays are only attempted before the provider
/// emits its first stream event.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub max_retry_after: Duration,
    pub jitter_seed: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(60),
            jitter_seed: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl RetryPolicy {
    /// Return the delay after a failed `attempt`, or `None` to stop retrying.
    pub fn delay_for(&self, attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        if attempt >= self.max_attempts {
            return None;
        }
        if let Some(server_wait) = retry_after {
            if server_wait > self.max_retry_after {
                return None;
            }
            return Some(server_wait);
        }
        let exponent = attempt.saturating_sub(1).min(20);
        let backoff = self
            .base_delay
            .saturating_mul(1u32 << exponent)
            .min(self.max_delay);
        let jitter = backoff.mul_f64(self.jitter_fraction(attempt));
        Some((backoff + jitter).min(self.max_delay))
    }

    fn jitter_fraction(&self, attempt: u32) -> f64 {
        let mut x = self
            .jitter_seed
            .wrapping_add((attempt as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        x ^= x >> 33;
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^= x >> 33;
        (x % 1000) as f64 / 4000.0
    }
}

pub(crate) fn call_model(
    model: &dyn Provider,
    prompt: &PromptContext,
    tools: &[ToolSpec],
    retry_policy: RetryPolicy,
    forward_stream: bool,
    emit: &mut dyn FnMut(AgentEvent),
    cancel: &AtomicBool,
) -> ModelCallResult {
    let mut attempt = 1u32;
    loop {
        let mut emitted_any = false;
        let mut forward = |event: ProviderEvent| {
            emitted_any = true;
            if !forward_stream {
                return;
            }
            emit(match event {
                ProviderEvent::TextDelta(text) => AgentEvent::AssistantDelta(text),
                ProviderEvent::ThinkingDelta(text) => AgentEvent::ThinkingDelta(text),
                ProviderEvent::ToolCallBegun { name } => AgentEvent::ToolCallPending { name },
            });
        };
        match model.stream_turn(prompt, tools, &mut forward, cancel) {
            StreamTerminal::Done(output) => return ModelCallResult::Done(output),
            StreamTerminal::Aborted(failed) => return ModelCallResult::Cancelled(failed),
            StreamTerminal::Error(failed) => {
                let delay = if failed.error.retryable && !emitted_any {
                    retry_policy.delay_for(attempt, failed.error.retry_after)
                } else {
                    None
                };
                let Some(wait) = delay else {
                    return ModelCallResult::Failed(failed);
                };
                emit(AgentEvent::Notice(format!(
                    "{},{:.1}s 后重试({}/{})",
                    failed.error,
                    wait.as_secs_f64(),
                    attempt,
                    retry_policy.max_attempts - 1
                )));
                let mut slept = Duration::ZERO;
                while slept < wait {
                    if cancel.load(Ordering::Relaxed) {
                        return ModelCallResult::Cancelled(FailedTurn::aborted());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    slept += Duration::from_millis(100);
                }
                attempt += 1;
            }
        }
    }
}

pub(crate) enum ModelCallResult {
    Done(TurnOutput),
    Cancelled(FailedTurn),
    Failed(FailedTurn),
}
