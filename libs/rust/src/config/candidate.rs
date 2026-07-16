//! Side-effect-free pre-commit configuration candidate validation.
//!
//! Validators are synchronous application callbacks. Each invocation receives owned defensive
//! copies of the candidate and redacted prior snapshot. A process-wide four-worker budget bounds
//! callbacks which ignore their deadline: a timed-out worker retains its permit until it actually
//! exits, so repeated reloads cannot create unbounded threads.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::Result;

/// Default overall deadline shared by one candidate generation's validators.
pub const DEFAULT_CANDIDATE_VALIDATION_TIMEOUT: Duration = Duration::from_secs(5);
/// Defensive maximum: configuration activation must never wait indefinitely.
pub const MAX_CANDIDATE_VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_VALIDATOR_WORKERS: usize = 4;
const MAX_DIAGNOSTIC_CHARS: usize = 256;

thread_local! {
    static IN_VALIDATOR_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// Whether this thread is currently executing an application candidate validator.
pub(crate) fn in_validator_callback() -> bool {
    IN_VALIDATOR_CALLBACK.with(Cell::get)
}

/// The lifecycle phase of one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationValidationPhase {
    /// The first candidate, before provider watches and application services start.
    Initial,
    /// Any candidate received after generation one committed.
    Reload,
}

/// One validator's deterministic verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigurationValidationResult {
    /// Accept this candidate.
    Accept,
    /// Reject with a stable machine-readable code and operator-safe diagnostic.
    Reject {
        /// Stable SCREAMING_SNAKE_CASE code.
        code: String,
        /// Human-readable diagnostic; bounded and sanitized by the runner.
        message: String,
    },
}

impl ConfigurationValidationResult {
    /// Accept the candidate.
    #[must_use]
    pub const fn accept() -> Self {
        Self::Accept
    }

    /// Reject the candidate. Invalid codes are converted to `VALIDATOR_FAILED` by the runner.
    #[must_use]
    pub fn reject(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Reject {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// A stable, operator-safe pre-commit failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationValidationError {
    /// Registration name of the validator which failed.
    pub validator: String,
    /// Stable machine-readable code.
    pub code: String,
    /// Sanitized diagnostic, at most 256 characters.
    pub message: String,
}

/// Synchronous, side-effect-free candidate validator.
pub trait ConfigurationCandidateValidator: Send + Sync + 'static {
    /// Validate owned defensive snapshots. `redacted_current` is `None` for initial load.
    fn validate(
        &self,
        candidate: Value,
        redacted_current: Option<Value>,
        phase: ConfigurationValidationPhase,
    ) -> Result<ConfigurationValidationResult>;
}

impl<F> ConfigurationCandidateValidator for F
where
    F: Fn(
            Value,
            Option<Value>,
            ConfigurationValidationPhase,
        ) -> Result<ConfigurationValidationResult>
        + Send
        + Sync
        + 'static,
{
    fn validate(
        &self,
        candidate: Value,
        redacted_current: Option<Value>,
        phase: ConfigurationValidationPhase,
    ) -> Result<ConfigurationValidationResult> {
        (self)(candidate, redacted_current, phase)
    }
}

/// One ordered builder registration.
#[derive(Clone)]
pub(crate) struct NamedValidator {
    pub(crate) name: String,
    pub(crate) validator: Arc<dyn ConfigurationCandidateValidator>,
}

struct WorkerBudget {
    available: Mutex<usize>,
}

impl WorkerBudget {
    fn try_acquire(&'static self) -> Option<WorkerPermit> {
        let mut available = self.available.lock().ok()?;
        if *available == 0 {
            return None;
        }
        *available -= 1;
        Some(WorkerPermit(self))
    }
}

struct WorkerPermit(&'static WorkerBudget);

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        if let Ok(mut available) = self.0.available.lock() {
            *available = (*available + 1).min(MAX_VALIDATOR_WORKERS);
        }
    }
}

fn worker_budget() -> &'static WorkerBudget {
    static BUDGET: OnceLock<WorkerBudget> = OnceLock::new();
    BUDGET.get_or_init(|| WorkerBudget {
        available: Mutex::new(MAX_VALIDATOR_WORKERS),
    })
}

/// Validate and normalize one configured overall deadline.
pub(crate) fn require_validation_timeout(timeout: Duration) -> Result<Duration> {
    if timeout.is_zero() || timeout > MAX_CANDIDATE_VALIDATION_TIMEOUT {
        return Err(crate::EdgeCommonsError::Config(
            "configuration validation timeout must be positive and at most 60 seconds".to_string(),
        ));
    }
    Ok(timeout)
}

/// Check the cross-language registration-name contract without compiling a regex per call.
pub(crate) fn valid_validator_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// Run one generation under a single deadline, retaining deterministic registration order.
pub(crate) fn validate_candidate(
    validators: &[NamedValidator],
    candidate: &Value,
    redacted_current: Option<&Value>,
    phase: ConfigurationValidationPhase,
    timeout: Duration,
) -> Vec<ConfigurationValidationError> {
    if validators.is_empty() {
        return Vec::new();
    }

    let deadline = Instant::now() + timeout;
    let tasks = Arc::new(Mutex::new(
        validators
            .iter()
            .cloned()
            .enumerate()
            .collect::<VecDeque<_>>(),
    ));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (results_tx, results_rx) = mpsc::channel();

    for worker_index in 0..validators.len().min(MAX_VALIDATOR_WORKERS) {
        let Some(permit) = worker_budget().try_acquire() else {
            break;
        };
        let tasks = tasks.clone();
        let cancelled = cancelled.clone();
        let results = results_tx.clone();
        let candidate = candidate.clone();
        let redacted_current = redacted_current.cloned();
        let spawn = std::thread::Builder::new()
            .name(format!("edgecommons-config-validator-{}", worker_index + 1))
            .spawn(move || {
                let _permit = permit;
                loop {
                    if cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    let task = tasks.lock().ok().and_then(|mut tasks| tasks.pop_front());
                    let Some((index, named)) = task else {
                        return;
                    };
                    if cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    let outcome = catch_unwind(AssertUnwindSafe(|| {
                        IN_VALIDATOR_CALLBACK.with(|active| active.set(true));
                        let result = named.validator.validate(
                            candidate.clone(),
                            redacted_current.clone(),
                            phase,
                        );
                        IN_VALIDATOR_CALLBACK.with(|active| active.set(false));
                        result
                    }));
                    // A panic bypasses the callback's normal reset.
                    IN_VALIDATOR_CALLBACK.with(|active| active.set(false));
                    let error = match outcome {
                        Ok(Ok(ConfigurationValidationResult::Accept)) => None,
                        Ok(Ok(ConfigurationValidationResult::Reject { code, message })) => {
                            if valid_rejection_code(&code) {
                                Some(ConfigurationValidationError {
                                    validator: named.name,
                                    code,
                                    message: sanitize(&message),
                                })
                            } else {
                                Some(failed(
                                    named.name,
                                    "validator returned an invalid rejection code",
                                ))
                            }
                        }
                        Ok(Err(error)) => Some(failed(named.name, &error.to_string())),
                        Err(_) => Some(failed(named.name, "validator panicked")),
                    };
                    if results.send((index, error)).is_err() {
                        return;
                    }
                }
            });
        // `permit` moved into the closure only if spawn succeeded; on failure the closure (and
        // permit) is dropped here. Unscheduled validators receive the normal timeout verdict.
        if spawn.is_err() {
            continue;
        }
    }
    drop(results_tx);

    let mut completed = HashMap::with_capacity(validators.len());
    while completed.len() < validators.len() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if remaining.is_zero() {
            break;
        }
        match results_rx.recv_timeout(remaining) {
            Ok((index, error)) => {
                completed.insert(index, error);
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    cancelled.store(true, Ordering::Release);

    validators
        .iter()
        .enumerate()
        .filter_map(|(index, named)| match completed.remove(&index) {
            Some(error) => error,
            None => Some(ConfigurationValidationError {
                validator: named.name.clone(),
                code: "VALIDATION_TIMEOUT".to_string(),
                message: "configuration validation exceeded its bounded deadline".to_string(),
            }),
        })
        .collect()
}

pub(crate) fn sanitize(message: &str) -> String {
    let safe: String = message
        .chars()
        .take(MAX_DIAGNOSTIC_CHARS)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    safe.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn failed(validator: String, detail: &str) -> ConfigurationValidationError {
    ConfigurationValidationError {
        validator,
        code: "VALIDATOR_FAILED".to_string(),
        message: sanitize(detail),
    }
}

fn valid_rejection_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_uppercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};

    fn named<F>(name: &str, validator: F) -> NamedValidator
    where
        F: ConfigurationCandidateValidator,
    {
        NamedValidator {
            name: name.to_string(),
            validator: Arc::new(validator),
        }
    }

    /// Serializes the tests that share the process-wide validator budget and
    /// waits until every permit has been returned before the caller proceeds.
    ///
    /// Cargo runs tests in parallel by default, and
    /// `repeated_timeouts_never_exceed_the_process_worker_budget` intentionally
    /// saturates the four-permit budget with detached, timed-out workers that
    /// release their permits asynchronously. Without this guard a co-scheduled
    /// test is starved of permits, so its validators spuriously time out and it
    /// fails only under CI's parallelism. Holding the returned guard for the
    /// test's duration prevents overlap; the drain loop reclaims any permit a
    /// prior test's not-yet-unwound worker still holds.
    fn budget_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static BUDGET_TEST_GUARD: Mutex<()> = Mutex::new(());
        let guard = BUDGET_TEST_GUARD
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut permits = Vec::new();
            while let Some(permit) = worker_budget().try_acquire() {
                permits.push(permit);
            }
            let acquired = permits.len();
            drop(permits); // restore the budget to full before proceeding
            if acquired == MAX_VALIDATOR_WORKERS {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "validator worker budget was never restored to full"
            );
            std::thread::yield_now();
        }
        guard
    }

    #[test]
    fn validators_receive_independent_owned_copies_and_redacted_prior() {
        let _budget = budget_test_guard();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let first = named(
            "mutator",
            |mut candidate: Value, mut prior: Option<Value>, _| {
                candidate["value"] = Value::String("changed".to_string());
                prior.as_mut().unwrap()["password"] = Value::String("changed".to_string());
                Ok(ConfigurationValidationResult::accept())
            },
        );
        let seen = observed.clone();
        let second = named(
            "observer",
            move |candidate: Value, prior: Option<Value>, _| {
                seen.lock().unwrap().push((
                    candidate["value"].clone(),
                    prior.unwrap()["password"].clone(),
                ));
                Ok(ConfigurationValidationResult::accept())
            },
        );

        let errors = validate_candidate(
            &[first, second],
            &serde_json::json!({"value": "original"}),
            Some(&serde_json::json!({"password": "***"})),
            ConfigurationValidationPhase::Reload,
            Duration::from_secs(1),
        );

        assert!(errors.is_empty());
        assert_eq!(
            *observed.lock().unwrap(),
            vec![(
                Value::String("original".to_string()),
                Value::String("***".to_string())
            )]
        );
    }

    #[test]
    fn repeated_timeouts_never_exceed_the_process_worker_budget() {
        let _budget = budget_test_guard();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));
        let max_live = Arc::new(AtomicUsize::new(0));
        let validator = {
            let gate = gate.clone();
            let entered = entered.clone();
            let live = live.clone();
            let max_live = max_live.clone();
            named("slow", move |_: Value, _: Option<Value>, _| {
                entered.fetch_add(1, Ordering::SeqCst);
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                max_live.fetch_max(now, Ordering::SeqCst);
                let (lock, condition) = &*gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = condition.wait(released).unwrap();
                }
                live.fetch_sub(1, Ordering::SeqCst);
                Ok(ConfigurationValidationResult::accept())
            })
        };

        for _ in 0..12 {
            let errors = validate_candidate(
                std::slice::from_ref(&validator),
                &serde_json::json!({}),
                None,
                ConfigurationValidationPhase::Reload,
                Duration::from_millis(5),
            );
            assert_eq!(errors[0].code, "VALIDATION_TIMEOUT");
        }
        assert_eq!(entered.load(Ordering::SeqCst), MAX_VALIDATOR_WORKERS);
        assert_eq!(max_live.load(Ordering::SeqCst), MAX_VALIDATOR_WORKERS);

        let (lock, condition) = &*gate;
        *lock.lock().unwrap() = true;
        condition.notify_all();
        let deadline = Instant::now() + Duration::from_secs(1);
        while live.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }
}
