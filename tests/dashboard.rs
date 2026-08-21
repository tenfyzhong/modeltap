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
fn dashboard_uses_the_selected_time_range_for_cost_totals() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();
    let panel = panels.iter().find(|panel| panel["id"] == 6).unwrap();

    assert_eq!(
        panel["targets"][0]["expr"],
        "sum by (site, model) (increase({__name__=~\"ai_proxy_cost(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\", currency=\"USD\"}[$__range]))"
    );
    assert_eq!(panel["targets"][0]["instant"], true);
}

#[test]
fn dashboard_groups_cumulative_token_breakdowns_by_agent_cli_site_model_and_type() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();

    let panel = panels.iter().find(|panel| panel["id"] == 5).unwrap();
    assert_eq!(
        panel["title"],
        "Cumulative tokens by agent_cli, site, model, and type"
    );
    assert!(
        panel["targets"][0]["expr"]
            .as_str()
            .unwrap()
            .starts_with("sum by (agent_cli, site, model, type) (")
    );
    assert_eq!(
        panel["targets"][0]["legendFormat"],
        "{{agent_cli}} / {{site}} / {{model}} / {{type}}"
    );
}

#[test]
fn dashboard_has_cumulative_cost_by_agent_cli_site_model_and_type() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panel = dashboard["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["title"] == "Cumulative cost by agent_cli, site, model, and type")
        .unwrap();

    assert!(
        panel["targets"][0]["expr"]
            .as_str()
            .unwrap()
            .starts_with("sum by (agent_cli, site, model, type) ({__name__=~\"ai_proxy_cost")
    );
    assert_eq!(
        panel["targets"][0]["legendFormat"],
        "{{agent_cli}} / {{site}} / {{model}} / {{type}}"
    );
}

#[test]
fn dashboard_places_selected_range_totals_at_the_bottom() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();
    let token_totals = panels.iter().find(|panel| panel["id"] == 7).unwrap();
    let cost_totals = panels.iter().find(|panel| panel["id"] == 6).unwrap();
    let highest_other_y = panels
        .iter()
        .filter(|panel| panel["id"] != 6 && panel["id"] != 7)
        .map(|panel| panel["gridPos"]["y"].as_i64().unwrap())
        .max()
        .unwrap();

    assert!(token_totals["gridPos"]["y"].as_i64().unwrap() > highest_other_y);
    assert!(cost_totals["gridPos"]["y"].as_i64().unwrap() > highest_other_y);
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
        "sum by (site, model) (increase({__name__=~\"ai_proxy_cost(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\", currency=\"USD\"}[$__range]))"
    );
    assert_eq!(panel["targets"][0]["legendFormat"], "{{site}} / {{model}}");
}

#[test]
fn dashboard_has_daily_token_and_cost_curves_per_model() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();

    for panel_id in [8, 9] {
        let panel = panels.iter().find(|panel| panel["id"] == panel_id).unwrap();
        let expression = panel["targets"][0]["expr"].as_str().unwrap();
        assert!(expression.starts_with("sum by (site, model) (increase("));
        assert!(expression.ends_with("[1d]))"));
        assert_eq!(panel["targets"][0]["legendFormat"], "{{site}} / {{model}}");
    }
}

#[test]
fn dashboard_has_daily_token_and_cost_curves_per_agent_cli() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();

    for panel_id in [10, 11] {
        let panel = panels.iter().find(|panel| panel["id"] == panel_id).unwrap();
        let expression = panel["targets"][0]["expr"].as_str().unwrap();
        assert!(expression.starts_with("sum by (agent_cli) (increase("));
        assert!(expression.contains("agent_cli=~\"$agent_cli\""));
        assert!(expression.ends_with("[1d]))"));
        assert_eq!(panel["targets"][0]["legendFormat"], "{{agent_cli}}");
    }
}

#[test]
fn dashboard_filters_metrics_by_agent_cli_and_shows_total_range_tokens() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let variables = dashboard["templating"]["list"].as_array().unwrap();
    assert!(
        variables
            .iter()
            .any(|variable| variable["name"] == "agent_cli")
    );

    let total_tokens = dashboard["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["id"] == 2)
        .unwrap();
    assert_eq!(
        total_tokens["targets"][0]["expr"],
        "sum(increase({__name__=~\"ai_proxy_tokens(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\"}[$__range]))"
    );
}

#[test]
fn dashboard_variables_use_explicit_label_value_queries() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let variables = dashboard["templating"]["list"].as_array().unwrap();

    let names = variables
        .iter()
        .map(|variable| variable["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["agent_cli", "site", "model"]);

    for (name, label) in [
        ("agent_cli", "agent_cli"),
        ("site", "site"),
        ("model", "model"),
    ] {
        let variable = variables
            .iter()
            .find(|variable| variable["name"] == name)
            .unwrap();
        let query = variable["query"]["query"].as_str().unwrap();
        assert_eq!(variable["query"]["qryType"], 1);
        assert_eq!(variable["definition"], query);
        assert!(query.starts_with("label_values({__name__=~\"ai_proxy_requests"));
        assert!(query.ends_with(&format!(", {label})")));
        assert!(variable["query"].get("label").is_none());
        assert!(variable["query"].get("metric").is_none());
    }
    assert!(
        variables
            .iter()
            .all(|variable| variable["name"] != "provider")
    );
    assert_eq!(
        variables[1]["query"]["query"],
        "label_values({__name__=~\"ai_proxy_requests(_total)?\", agent_cli=~\"$agent_cli\"}, site)"
    );
    assert_eq!(
        variables[2]["query"]["query"],
        "label_values({__name__=~\"ai_proxy_requests(_total)?\", agent_cli=~\"$agent_cli\", site=~\"$site\"}, model)"
    );
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
