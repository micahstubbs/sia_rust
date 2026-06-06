//! Online verification of bundled Nebius profile model slugs (issue #83).
//!
//! Nebius Token Factory version-stamps its model slugs and can rename/retire
//! them. The bundled `*-nebius-*` profiles each pin an exact `model` slug; if a
//! slug drifts, runs fail at call time. This test is the "re-verify before the
//! hackathon" safety net: it GETs the live OpenAI-compatible `/v1/models`
//! catalog and asserts that EVERY bundled Nebius profile's `model` is present.
//!
//! It is `#[ignore]` by default and additionally returns early if
//! `NEBIUS_API_KEY` is unset, so offline CI is entirely unaffected. Run it
//! explicitly with a key:
//!
//! ```bash
//! NEBIUS_API_KEY=sk-... cargo test --features llm --ignored nebius
//! ```
//!
//! The whole file is gated behind the `llm` feature (which provides the
//! optional `reqwest` blocking dependency used here).

#![cfg(feature = "llm")]

use std::collections::BTreeSet;

use sia::llm::provider_mapping::base_url_for;
use sia::profiles::{available_profiles, load_meta_agent_profile, load_target_agent_profile};
use sia::providers::Provider;

/// (model_slug, provider) for one bundled profile, regardless of role.
struct ProfileModel {
    profile_id: String,
    model: String,
    provider: Provider,
}

/// Load every bundled profile (trying target then meta role) and return the
/// (profile_id, model, provider) tuple for those whose provider is `nebius`.
fn bundled_nebius_models() -> Vec<ProfileModel> {
    let mut out = Vec::new();
    for name in available_profiles() {
        // A profile is either a target or a meta profile; try both shapes.
        let resolved = load_target_agent_profile(&name)
            .map(|p| (p.profile_id, p.model, p.provider))
            .or_else(|_| {
                load_meta_agent_profile(&name).map(|p| (p.profile_id, p.model, p.provider))
            });
        if let Ok((profile_id, model, provider)) = resolved {
            if provider.provider_id == "nebius" {
                out.push(ProfileModel {
                    profile_id,
                    model,
                    provider,
                });
            }
        }
    }
    out
}

/// GET `{base_url}/models` and return the set of model ids from the
/// OpenAI-compatible `{ "data": [ { "id": ... } ] }` response body.
fn fetch_live_model_ids(base_url: &str, api_key: &str) -> BTreeSet<String> {
    // base_url from the provider ends in a trailing slash (".../v1/"); join
    // without doubling it.
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let resp = reqwest::blocking::Client::new()
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .unwrap_or_else(|e| panic!("GET {url} failed: {e}"));
    assert!(
        resp.status().is_success(),
        "GET {url} returned non-success status {}",
        resp.status()
    );
    let body: serde_json::Value = resp
        .json()
        .unwrap_or_else(|e| panic!("response from {url} was not JSON: {e}"));
    body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("response from {url} has no `data` array: {body}"))
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect()
}

/// Every bundled Nebius profile's `model` slug must exist in the live catalog.
///
/// Ignored by default (networked + needs a key). Skips cleanly if the key is
/// unset so an accidental `--ignored` run offline does not hard-fail.
#[test]
#[ignore = "requires NEBIUS_API_KEY and network access to the live /v1/models catalog"]
fn bundled_nebius_profile_slugs_exist_in_live_catalog() {
    let Ok(api_key) = std::env::var("NEBIUS_API_KEY") else {
        eprintln!("NEBIUS_API_KEY not set; skipping live Nebius catalog check.");
        return;
    };

    let profiles = bundled_nebius_models();
    assert!(
        !profiles.is_empty(),
        "expected at least one bundled nebius profile to verify"
    );

    // All bundled nebius profiles share one provider/base_url; fetch once.
    let base_url = base_url_for(&profiles[0].provider);
    let live_ids = fetch_live_model_ids(&base_url, &api_key);
    assert!(
        !live_ids.is_empty(),
        "live catalog at {base_url} returned no model ids"
    );

    // Collect every (profile_id, model) whose slug is absent from the catalog.
    let missing: Vec<String> = profiles
        .iter()
        .filter(|p| !live_ids.contains(&p.model))
        .map(|p| format!("{} -> {}", p.profile_id, p.model))
        .collect();

    assert!(
        missing.is_empty(),
        "these bundled Nebius profile slugs are NOT in the live /v1/models catalog \
         (update the profile or confirm the slug in the Token Factory console):\n  {}\n\n\
         Live catalog ids:\n  {}",
        missing.join("\n  "),
        live_ids.into_iter().collect::<Vec<_>>().join("\n  ")
    );
}
