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
        .filter(|panel| ![6, 7].contains(&panel["id"].as_i64().unwrap()))
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

    let agent_cli = variables
        .iter()
        .find(|variable| variable["name"] == "agent_cli")
        .unwrap();
    assert_eq!(agent_cli["query"]["query"], "label_values(agent_cli)");
    assert_eq!(agent_cli["definition"], "label_values(agent_cli)");
    assert_eq!(agent_cli["query"]["qryType"], 1);

    for (name, label) in [("site", "site"), ("model", "model")] {
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

    for panel_id in [2, 5, 7, 15] {
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

#[test]
fn dashboard_shows_p95_modeltap_chunk_processing_duration_in_microseconds() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panel = dashboard["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["title"] == "P95 ModelTap chunk processing duration by site")
        .expect("chunk processing duration panel exists");

    assert_eq!(panel["fieldConfig"]["defaults"]["unit"], "µs");
    assert_eq!(panel["gridPos"]["w"], 12);
    assert_eq!(panel["gridPos"]["x"], 0);
    assert_eq!(panel["gridPos"]["y"], 47);
    assert_eq!(
        panel["targets"][0]["expr"],
        "histogram_quantile(0.95, sum by (le, site) (rate(ai_proxy_local_processing_duration_microseconds_bucket{site=~\"$site\"}[$__rate_interval])))"
    );
    assert_eq!(panel["targets"][0]["legendFormat"], "{{site}}");
}

#[test]
fn dashboard_shows_average_modeltap_chunk_processing_duration_in_microseconds() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panel = dashboard["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["title"] == "Average ModelTap chunk processing duration by site")
        .expect("average chunk processing duration panel exists");

    assert_eq!(panel["fieldConfig"]["defaults"]["unit"], "µs");
    assert_eq!(panel["gridPos"]["w"], 12);
    assert_eq!(panel["gridPos"]["x"], 12);
    assert_eq!(panel["gridPos"]["y"], 47);
    assert_eq!(panel["targets"][0]["legendFormat"], "{{site}}");
    assert_eq!(
        panel["targets"][0]["expr"],
        "sum by (site) (rate(ai_proxy_local_processing_duration_microseconds_sum{site=~\"$site\"}[$__rate_interval])) / sum by (site) (rate(ai_proxy_local_processing_duration_microseconds_count{site=~\"$site\"}[$__rate_interval]))"
    );
}

#[test]
fn dashboard_shows_qps_by_model_using_rate_so_idle_periods_drop_to_zero() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();
    let panel = panels.iter().find(|panel| panel["id"] == 4).unwrap();

    assert_eq!(panel["title"], "QPS by model");
    assert_eq!(panel["fieldConfig"]["defaults"]["unit"], "ops");
    assert_eq!(
        panel["targets"][0]["expr"],
        "sum by (site, model) (rate({__name__=~\"ai_proxy_requests(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\"}[$__rate_interval]))"
    );
    assert_eq!(panel["targets"][0]["legendFormat"], "{{site}} / {{model}}");
}

#[test]
fn dashboard_shows_qps_stat_using_rate_so_idle_periods_drop_to_zero() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();
    let panel = panels.iter().find(|panel| panel["id"] == 1).unwrap();

    assert_eq!(panel["title"], "QPS");
    assert_eq!(panel["fieldConfig"]["defaults"]["unit"], "ops");
    assert_eq!(
        panel["targets"][0]["expr"],
        "sum(rate({__name__=~\"ai_proxy_requests(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\"}[$__rate_interval]))"
    );
}

#[test]
fn dashboard_has_realtime_token_and_cost_breakdowns_by_agent_cli_site_model_and_type() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();

    let token_panel = panels
        .iter()
        .find(|panel| panel["id"] == 15)
        .expect("realtime token panel exists");
    assert_eq!(
        token_panel["title"],
        "Tokens by agent_cli, site, model, and type"
    );
    assert_eq!(token_panel["fieldConfig"]["defaults"]["unit"], "short");
    assert_eq!(
        token_panel["targets"][0]["expr"],
        "sum by (agent_cli, site, model, type) (rate({__name__=~\"ai_proxy_tokens(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\"}[$__rate_interval]))"
    );
    assert_eq!(
        token_panel["targets"][0]["legendFormat"],
        "{{agent_cli}} / {{site}} / {{model}} / {{type}}"
    );

    let cost_panel = panels
        .iter()
        .find(|panel| panel["id"] == 16)
        .expect("realtime cost panel exists");
    assert_eq!(
        cost_panel["title"],
        "Cost by agent_cli, site, model, and type (USD)"
    );
    assert_eq!(cost_panel["fieldConfig"]["defaults"]["unit"], "currencyUSD");
    assert_eq!(
        cost_panel["targets"][0]["expr"],
        "sum by (agent_cli, site, model, type) (rate({__name__=~\"ai_proxy_cost(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\", currency=\"USD\"}[$__rate_interval]))"
    );
    assert_eq!(
        cost_panel["targets"][0]["legendFormat"],
        "{{agent_cli}} / {{site}} / {{model}} / {{type}}"
    );
}

#[test]
fn dashboard_has_summary_panels_matching_the_dashboard_preview() {
    let dashboard: Value = serde_json::from_str(include_str!("../grafana/modeltap-dashboard.json"))
        .expect("dashboard JSON is valid");
    let panels = dashboard["panels"].as_array().unwrap();

    let top_agent = panels
        .iter()
        .find(|panel| panel["title"] == "Top agent by estimated cost (USD)")
        .expect("top agent panel exists");
    assert_eq!(top_agent["type"], "stat");
    assert_eq!(top_agent["fieldConfig"]["defaults"]["unit"], "currencyUSD");
    assert_eq!(
        top_agent["targets"][0]["expr"],
        "topk(1, sum by (agent_cli) (increase({__name__=~\"ai_proxy_cost(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\", currency=\"USD\"}[$__range])))"
    );

    let tokens_by_agent = panels
        .iter()
        .find(|panel| panel["title"] == "Tokens by agent CLI in selected range")
        .expect("token-by-agent bar gauge exists");
    assert_eq!(tokens_by_agent["type"], "bargauge");
    assert_eq!(tokens_by_agent["fieldConfig"]["defaults"]["unit"], "short");
    assert_eq!(
        tokens_by_agent["targets"][0]["expr"],
        "sum by (agent_cli) (increase({__name__=~\"ai_proxy_tokens(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\"}[$__range]))"
    );

    let cost_by_model = panels
        .iter()
        .find(|panel| panel["title"] == "Estimated cost by model in selected range (USD)")
        .expect("cost-by-model bar gauge exists");
    assert_eq!(cost_by_model["type"], "bargauge");
    assert_eq!(cost_by_model["fieldConfig"]["defaults"]["unit"], "currencyUSD");
    assert_eq!(
        cost_by_model["targets"][0]["expr"],
        "topk(10, sum by (model) (increase({__name__=~\"ai_proxy_cost(_total)?\", site=~\"$site\", model=~\"$model\", agent_cli=~\"$agent_cli\", currency=\"USD\"}[$__range])))"
    );
}
