//! Oracle: the graph was ACTUALLY computed on the execution provider.
//!
//! # Why a separate layer
//! `error_on_failure()` on an EP catches EXACTLY one failure: the provider did not
//! register. The case where the EP came up but got no nodes, and
//! the whole graph ran on the CPU, passes this check straight through: the session
//! is created, numbers are computed, no errors. It could be told apart from success
//! only indirectly, by speed, i.e. by eye and not in a test.
//!
//! The only known direct answer comes from the ONNX Runtime profile: in it
//! EACH node event carries the name of the provider that executed the node.
//! Hence the whole module: turn the profile into a census of nodes per
//! provider, from which one can claim (and assert) where things were computed.
//!
//! ORT event format: an array of objects; node events have `"cat": "Node"`, the
//! provider name is in `args.provider`. A single node gets SEVERAL events
//! (`_fence_before`, `_kernel_time`, `_fence_after`), with one set for
//! EACH run, so the census is taken by node NAMES, not by events:
//! otherwise the number would depend on how many times the model was run, and 4158 would
//! mean nothing.

use crate::embeddings::EmbeddingError;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

/// Name of the MIGraphX EP in the ORT profile: what the provider tags its nodes with.
pub const MIGRAPHX_EP: &str = "MIGraphXExecutionProvider";
/// Name of the DirectML EP in the ORT profile (the Windows GPU path).
///
/// 🚨 Not `DirectMLExecutionProvider`: ORT uses the short name, and a typo
/// here would go completely unnoticed: the census would just return zero nodes, i.e.
/// the 'GPU really works' gate would go red on a healthy machine.
pub const DIRECTML_EP: &str = "DmlExecutionProvider";
/// Name of the CPU EP in the ORT profile. Shape nodes (Shape/Reshape/Cast) stay on it
/// even with a fully healthy GPU path; there are a handful of them, and that is normal.
pub const CPU_EP: &str = "CPUExecutionProvider";

/// How many DISTINCT graph nodes each provider executed.
///
/// Invariant: the census is non-empty. An empty one (a profile without provider tags) means
/// 'not looked at', not 'nothing on GPU', and it must not be indistinguishable from
/// an honest zero, so the constructors return an error in that case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCensus {
    per_provider: BTreeMap<String, usize>,
}

impl ProviderCensus {
    /// Census from the file returned by `end_profiling()`.
    pub fn from_profile_file(path: &Path) -> Result<Self, EmbeddingError> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            EmbeddingError::model_init(format!(
                "cannot read ORT profile at {}: {e}",
                path.display()
            ))
        })?;
        Self::from_profile_json(&raw)
    }

    /// Census from the profile contents.
    ///
    /// Refusals instead of silent fitting: a non-array and a profile without a single
    /// provider tag are errors. The second matters especially: silently returning an empty
    /// census means handing the caller a zero, which they will read as
    /// 'everything ran on CPU', when in fact the profile is simply about something else.
    pub fn from_profile_json(raw: &str) -> Result<Self, EmbeddingError> {
        let events: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| EmbeddingError::model_init(format!("malformed ORT profile: {e}")))?;
        let events = events.as_array().ok_or_else(|| {
            EmbeddingError::model_init("ORT profile is not a JSON array of events")
        })?;

        // Node name → provider. A set, not a counter, because the events
        // of one node repeat on every run.
        let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for event in events {
            if event.get("cat").and_then(|c| c.as_str()) != Some("Node") {
                continue;
            }
            let Some(provider) = event
                .get("args")
                .and_then(|a| a.get("provider"))
                .and_then(|p| p.as_str())
            else {
                continue;
            };
            let Some(name) = event.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            seen.entry(node_name(name).to_string())
                .or_default()
                .insert(provider.to_string());
        }

        if seen.is_empty() {
            return Err(EmbeddingError::model_init(
                "ORT profile carries no provider-tagged node events — \
                 profiling was probably not enabled for this session",
            ));
        }

        let mut per_provider: BTreeMap<String, usize> = BTreeMap::new();
        for providers in seen.values() {
            // One node is executed by one provider; if the profile claims
            // otherwise, count the node for each of them: silently picking the first one would
            // hide what we do not understand.
            for provider in providers {
                *per_provider.entry(provider.clone()).or_insert(0) += 1;
            }
        }
        Ok(Self { per_provider })
    }

    pub fn per_provider(&self) -> &BTreeMap<String, usize> {
        &self.per_provider
    }

    pub fn nodes_on(&self, provider: &str) -> usize {
        self.per_provider.get(provider).copied().unwrap_or(0)
    }

    pub fn total_nodes(&self) -> usize {
        self.per_provider.values().sum()
    }

    /// Share of nodes that went to the provider, in [0, 1].
    ///
    /// The denominator is always non-empty (see the type invariant); division by zero
    /// cannot happen here by construction.
    pub fn share_on(&self, provider: &str) -> f64 {
        self.nodes_on(provider) as f64 / self.total_nodes() as f64
    }
}

impl fmt::Display for ProviderCensus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .per_provider
            .iter()
            .map(|(provider, count)| format!("{provider}={count}"))
            .collect();
        write!(f, "{} nodes: {}", self.total_nodes(), parts.join(", "))
    }
}

/// Node name without the suffix ORT uses to mark the event PHASE.
///
/// `Add_12_kernel_time`, `Add_12_fence_before` and `Add_12_fence_after` are one and
/// the same graph node; without stripping the suffix it would be counted three times.
fn node_name(event_name: &str) -> &str {
    for suffix in ["_kernel_time", "_fence_before", "_fence_after"] {
        if let Some(stripped) = event_name.strip_suffix(suffix) {
            return stripped;
        }
    }
    event_name
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile in the form ORT writes: three events per node, two runs.
    fn profile(nodes: &[(&str, &str)], runs: usize) -> String {
        let mut events = vec![serde_json::json!({
            "cat": "Session", "name": "model_loading_uri", "dur": 1
        })];
        for _ in 0..runs {
            for (name, provider) in nodes {
                for phase in ["_fence_before", "_kernel_time", "_fence_after"] {
                    events.push(serde_json::json!({
                        "cat": "Node",
                        "name": format!("{name}{phase}"),
                        "dur": 7,
                        "args": {"provider": provider, "op_name": "Add"},
                    }));
                }
            }
        }
        serde_json::Value::Array(events).to_string()
    }

    /// The census counts NODES, not events: three events per node and two runs
    /// do not turn two nodes into twelve.
    ///
    /// This is the key property: if the number tracked the run count, any
    /// threshold on the share of GPU nodes would depend on how many times the model was run.
    #[test]
    fn census_counts_nodes_not_events() {
        let raw = profile(&[("Add_1", MIGRAPHX_EP), ("Shape_2", CPU_EP)], 2);
        let census = ProviderCensus::from_profile_json(&raw).unwrap();
        assert_eq!(census.nodes_on(MIGRAPHX_EP), 1);
        assert_eq!(census.nodes_on(CPU_EP), 1);
        assert_eq!(census.total_nodes(), 2);
        assert!((census.share_on(MIGRAPHX_EP) - 0.5).abs() < 1e-9);
    }

    /// A silent fallback to CPU is visible in the census: exactly the class this
    /// module was created for.
    #[test]
    fn census_sees_silent_cpu_fallback() {
        let raw = profile(&[("Add_1", CPU_EP), ("MatMul_2", CPU_EP)], 1);
        let census = ProviderCensus::from_profile_json(&raw).unwrap();
        assert_eq!(census.nodes_on(MIGRAPHX_EP), 0);
        assert_eq!(census.share_on(CPU_EP), 1.0);
    }

    /// 'Not looked at' ≠ 'nothing on GPU': a profile without provider tags must
    /// be a refusal, otherwise zero GPU nodes cannot be told apart from missing data.
    #[test]
    fn census_rejects_profile_without_provider_tags() {
        let raw = serde_json::json!([
            {"cat": "Session", "name": "session_initialization", "dur": 1},
            {"cat": "Node", "name": "Add_1_kernel_time", "dur": 2, "args": {"op_name": "Add"}},
        ])
        .to_string();
        let err = ProviderCensus::from_profile_json(&raw).unwrap_err();
        assert!(
            err.to_string().contains("no provider-tagged node events"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn census_rejects_malformed_profile() {
        assert!(ProviderCensus::from_profile_json("{}").is_err());
        assert!(ProviderCensus::from_profile_json("not json").is_err());
    }
}
