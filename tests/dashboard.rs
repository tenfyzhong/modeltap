use serde_json::Value;

#[test]
fn dashboard_prompts_for_a_prometheus_datasource_at_import_time() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");

    assert_eq!(dashboard["__inputs"][0]["name"], "DS_PROMETHEUS");
    assert!(
        dashboard["templating"]["list"]
            .as_array()
            .unwrap()
            .iter()
            .all(|variable| variable["name"] != "datasource")
    );
    assert!(
        dashboard["panels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|panel| panel["datasource"]["uid"] == "${DS_PROMETHEUS}")
    );
}

#[test]
fn dashboard_displays_counters_from_their_first_exported_sample() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");

    for panel in dashboard["panels"].as_array().unwrap() {
        let expression = panel["targets"][0]["expr"].as_str().unwrap();
        assert!(
            !expression.contains("increase(") && !expression.contains("rate("),
            "panel {} hides a counter's first sample: {expression}",
            panel["id"]
        );
    }
}

#[test]
fn dashboard_groups_token_breakdowns_by_site_model_and_type() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();

    for panel_id in [5, 7] {
        let panel = panels.iter().find(|panel| panel["id"] == panel_id).unwrap();
        assert!(
            panel["targets"][0]["expr"]
                .as_str()
                .unwrap()
                .starts_with("sum by (site, model, type) (")
        );
    }
}

#[test]
fn dashboard_groups_cost_by_site_and_model() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panel = dashboard["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["id"] == 6)
        .unwrap();

    assert_eq!(
        panel["targets"][0]["expr"],
        "sum by (site, model) ({__name__=~\"ai_proxy_cost(_total)?\", site=~\"$site\", provider=~\"$provider\", model=~\"$model\", currency=\"USD\"})"
    );
    assert_eq!(panel["targets"][0]["legendFormat"], "{{site}} / {{model}}");
}

#[test]
fn dashboard_abbreviates_token_values_with_k_m_and_b_suffixes() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();

    for panel_id in [2, 5, 7] {
        let panel = panels.iter().find(|panel| panel["id"] == panel_id).unwrap();
        assert_eq!(
            panel["fieldConfig"]["defaults"]["unit"], "short",
            "token panel {panel_id} must use Grafana's K/M/B abbreviation"
        );
        assert!(
            panel["fieldConfig"]["overrides"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|override_config| {
                    override_config["properties"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .all(|property| property["value"] != "tokens"),
            "token panel {panel_id} overrides the short unit"
        );
    }
}
