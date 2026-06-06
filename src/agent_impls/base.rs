//! Agent-implementation registry. Port of `sia/agent_impls/base.py`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::{SiaError, SiaResult};
use crate::providers::Provider;

/// Arguments passed to an agent-impl runner.
#[derive(Debug, Clone, PartialEq)]
pub struct RunArgs {
    pub model_name: String,
    pub max_turns: String,
    pub prompt: String,
    pub agent_working_directory: String,
    pub provider: Option<Provider>,
}

/// A registered agent-impl runner. Synchronous: the orchestrator blocks on the
/// agent anyway (Python wrapped each call in `asyncio.run`).
pub type Runner = Arc<dyn Fn(&RunArgs) -> SiaResult<()> + Send + Sync>;

fn registry() -> &'static Mutex<HashMap<String, Runner>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Runner>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut m: HashMap<String, Runner> = HashMap::new();
        m.insert(
            "claude".to_string(),
            Arc::new(super::claude::run_agent_claude),
        );
        m.insert(
            "openhands".to_string(),
            Arc::new(super::openhands::run_agent_openhands),
        );
        m.insert(
            "pydantic-ai".to_string(),
            Arc::new(super::pydantic_ai::run_agent_pydantic_ai),
        );
        Mutex::new(m)
    })
}

/// Register an agent-impl runner under `name`.
pub fn register(name: &str, runner: Runner) {
    registry().lock().unwrap().insert(name.to_string(), runner);
}

/// Ids of all registered agent impls.
pub fn available_agent_impls() -> Vec<String> {
    registry().lock().unwrap().keys().cloned().collect()
}

/// Return the runner registered under `name` (errs if unknown).
pub fn get_agent_impl(name: &str) -> SiaResult<Runner> {
    let reg = registry().lock().unwrap();
    match reg.get(name) {
        Some(r) => Ok(r.clone()),
        None => {
            let available = reg.keys().cloned().collect::<Vec<_>>().join(", ");
            Err(SiaError::new(format!(
                "Unknown agent impl: {name}. Available: {available}"
            )))
        }
    }
}

/// Dispatch to the named agent impl.
pub fn run_agent(
    model_name: &str,
    max_turns: &str,
    prompt: &str,
    agent_working_directory: &str,
    agent_impl: &str,
    provider: Option<Provider>,
) -> SiaResult<()> {
    let runner = get_agent_impl(agent_impl)?;
    let args = RunArgs {
        model_name: model_name.to_string(),
        max_turns: max_turns.to_string(),
        prompt: prompt.to_string(),
        agent_working_directory: agent_working_directory.to_string(),
        provider,
    };
    runner(&args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::load_provider;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn test_registry_lists_builtin_agent_impls() {
        let names: std::collections::HashSet<String> =
            available_agent_impls().into_iter().collect();
        for expected in ["claude", "openhands", "pydantic-ai"] {
            assert!(names.contains(expected), "missing impl {expected}");
        }
    }

    #[test]
    fn test_get_agent_impl_returns_callable() {
        assert!(get_agent_impl("claude").is_ok());
        assert!(get_agent_impl("pydantic-ai").is_ok());
    }

    #[test]
    fn test_get_agent_impl_unknown_raises() {
        assert!(get_agent_impl("does-not-exist").is_err());
    }

    #[test]
    fn test_run_agent_threads_provider_to_agent_impl() {
        static CAPTURED: OnceLock<StdMutex<Option<Option<Provider>>>> = OnceLock::new();
        let cell = CAPTURED.get_or_init(|| StdMutex::new(None));
        let runner: Runner = Arc::new(|args: &RunArgs| {
            *CAPTURED.get().unwrap().lock().unwrap() = Some(args.provider.clone());
            Ok(())
        });
        register("capture-test", runner);
        let nebius = load_provider("nebius").unwrap();
        run_agent("m", "5", "p", "/tmp", "capture-test", Some(nebius.clone())).unwrap();
        assert_eq!(cell.lock().unwrap().clone(), Some(Some(nebius)));
    }
}
