//! The terminal renderer against rich and typer: every fixture in `golden/fixtures/cli` (written by
//! `golden/make_cli_help.py`) must come out byte for byte.

use serde_json::Value;
use tellfold_cli::term::{
    cell_len, render_error, render_help, render_text, render_text_soft, Column, CommandHelp, Justify, Overflow, Panel,
    ParamHelp, Table, Text, HEAVY_HEAD,
};

fn fixture(name: &str) -> Value {
    let path = format!("{}/../../golden/fixtures/cli/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).expect("fixture is JSON")
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}")).to_string()
}

fn opt_s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

fn opt_n(v: &Value) -> Option<usize> {
    v.as_u64().map(|n| n as usize)
}

/// Collects mismatches so one run shows all of them, with the first differing line of each.
#[derive(Default)]
struct Check {
    failures: Vec<String>,
    count: usize,
}

impl Check {
    fn eq(&mut self, label: &str, got: &str, want: &str) {
        self.count += 1;
        if got != want {
            let line = got.lines().zip(want.lines()).position(|(a, b)| a != b);
            let detail = match line {
                Some(i) => format!(
                    "line {i}:\n   got: {:?}\n  want: {:?}",
                    got.lines().nth(i).unwrap_or(""),
                    want.lines().nth(i).unwrap_or("")
                ),
                None => format!("{} lines vs {} lines", got.lines().count(), want.lines().count()),
            };
            self.failures.push(format!("{label}: {detail}\n--- got\n{got}--- want\n{want}"));
        }
    }

    fn finish(self) {
        assert!(self.count > 0, "no fixtures were compared");
        eprintln!("{} renders compared", self.count);
        assert!(
            self.failures.is_empty(),
            "{} of {} renders differ:\n{}",
            self.failures.len(),
            self.count,
            self.failures.join("\n\n")
        );
    }
}

fn param(v: &Value) -> ParamHelp {
    ParamHelp {
        required: v["required"].as_bool().expect("bool"),
        long: s(&v["long"]),
        short: s(&v["short"]),
        secondary_long: s(&v["secondary_long"]),
        secondary_short: s(&v["secondary_short"]),
        types: s(&v["types"]),
        help: s(&v["help"]),
        default: opt_s(&v["default"]),
        envvar: opt_s(&v["envvar"]),
    }
}

#[test]
fn help_screens_match_typer() {
    let mut check = Check::default();
    for case in fixture("help.json").as_array().expect("list") {
        let data = &case["data"];
        let help = CommandHelp {
            usage: s(&data["usage"]),
            help: s(&data["help"]),
            arguments: data["arguments"].as_array().expect("list").iter().map(param).collect(),
            options: data["options"].as_array().expect("list").iter().map(param).collect(),
            commands: data["commands"].as_array().expect("list").iter().map(|c| (s(&c[0]), s(&c[1]))).collect(),
        };
        for (width, want) in case["renders"].as_object().expect("map") {
            let got = render_help(&help, width.parse().expect("width"));
            check.eq(&format!("help {:?} at {width}", s(&case["command"])), &got, &s(want));
        }
    }
    check.finish();
}

#[test]
fn usage_errors_match_typer() {
    let mut check = Check::default();
    for case in fixture("errors.json").as_array().expect("list") {
        let data = &case["data"];
        for (width, want) in case["renders"].as_object().expect("map") {
            let got = render_error(
                &s(&data["usage"]),
                &s(&data["command_path"]),
                &s(&data["message"]),
                width.parse().expect("width"),
            );
            check.eq(&format!("error {:?} at {width}", s(&case["case"])), &got, &s(want));
        }
    }
    check.finish();
}

fn justify(v: &Value) -> Justify {
    match v.as_str().unwrap_or("left") {
        "left" => Justify::Left,
        "center" => Justify::Center,
        "right" => Justify::Right,
        other => panic!("justify {other}"),
    }
}

fn overflow(v: &Value) -> Overflow {
    match v.as_str().unwrap_or("ellipsis") {
        "fold" => Overflow::Fold,
        "crop" => Overflow::Crop,
        "ellipsis" => Overflow::Ellipsis,
        other => panic!("overflow {other}"),
    }
}

fn table(spec: &Value) -> Table {
    let mut table = Table::new()
        .title(spec["title"].as_str().unwrap_or(""))
        .title_justify(justify(&spec["title_justify"]))
        .expand(spec["expand"].as_bool().unwrap_or(false))
        .show_header(spec["show_header"].as_bool().unwrap_or(true))
        .show_edge(spec["show_edge"].as_bool().unwrap_or(true))
        .pad_edge(spec["pad_edge"].as_bool().unwrap_or(true))
        .collapse_padding(spec["collapse_padding"].as_bool().unwrap_or(false));
    if spec.get("box").is_some_and(Value::is_null) {
        table = table.box_chars(None);
    } else {
        table = table.box_chars(Some(HEAVY_HEAD));
    }
    if let Some(p) = spec["padding"].as_array() {
        let n = |i: usize| p[i].as_u64().expect("int") as usize;
        table = table.padding((n(0), n(1), n(2), n(3)));
    }
    for col in spec["columns"].as_array().expect("list") {
        let mut column = Column::new(&s(&col["header"]))
            .justify(justify(&col["justify"]))
            .overflow(overflow(&col["overflow"]))
            .no_wrap(col["no_wrap"].as_bool().unwrap_or(false));
        if let Some(w) = opt_n(&col["width"]) {
            column = column.width(w);
        }
        if let Some(w) = opt_n(&col["min_width"]) {
            column = column.min_width(w);
        }
        if let Some(w) = opt_n(&col["max_width"]) {
            column = column.max_width(w);
        }
        if let Some(r) = opt_n(&col["ratio"]) {
            column = column.ratio(r);
        }
        table = table.with_column(column);
    }
    for row in spec["rows"].as_array().expect("list") {
        table.add_row(row.as_array().expect("list").iter().map(|c| Text::plain(&s(c))).collect());
    }
    table
}

#[test]
fn tables_match_rich() {
    let mut check = Check::default();
    for spec in fixture("render.json")["tables"].as_array().expect("list") {
        let table = table(spec);
        for (width, want) in spec["renders"].as_object().expect("map") {
            let got = table.render(width.parse().expect("width"), false);
            check.eq(&format!("table {} at {width}", s(&spec["name"])), &got, &s(want));
        }
    }
    check.finish();
}

#[test]
fn panels_match_rich() {
    let mut check = Check::default();
    for spec in fixture("render.json")["panels"].as_array().expect("list") {
        let panel = Panel::new(Text::plain(&s(&spec["body"]))).title(&s(&spec["title"])).border_style("cyan");
        for (width, want) in spec["renders"].as_object().expect("map") {
            let got = panel.render(width.parse().expect("width"), false);
            check.eq(&format!("panel {:?} at {width}", s(&spec["title"])), &got, &s(want));
        }
    }
    check.finish();
}

#[test]
fn prints_match_rich() {
    let mut check = Check::default();
    for spec in fixture("render.json")["prints"].as_array().expect("list") {
        let text = Text::from_markup(&s(&spec["text"]));
        for (width, want) in spec["wrapped"].as_object().expect("map") {
            let got = render_text(&text, width.parse().expect("width"), false);
            check.eq(&format!("print {:?} at {width}", s(&spec["text"])), &got, &s(want));
        }
        check.eq(
            &format!("soft print {:?}", s(&spec["text"])),
            &render_text_soft(&text, false),
            &s(&spec["soft_wrap"]),
        );
    }
    check.finish();
}

#[test]
fn cell_lengths_match_rich() {
    for case in fixture("render.json")["cell_len"].as_array().expect("list") {
        assert_eq!(cell_len(&s(&case["text"])), case["cells"].as_u64().expect("int") as usize, "{case}");
    }
}

/// A text from `[text, style]` pairs, like `Text.assemble`.
fn assembled(parts: &Value) -> Text {
    let mut text = Text::new();
    for part in parts.as_array().expect("list") {
        text.push(&s(&part[0]), &s(&part[1]));
    }
    text
}

#[test]
fn styled_output_matches_rich() {
    let color = &fixture("render.json")["color"];
    let mut check = Check::default();
    let spec = &color["table"]["spec"];
    let mut table = Table::new().title(&s(&spec["title"]));
    for header in spec["columns"].as_array().expect("list") {
        table.add_column_with(&s(header), Justify::Left, Overflow::Fold);
    }
    for row in spec["rows"].as_array().expect("list") {
        table.add_row(row.as_array().expect("list").iter().map(assembled).collect());
    }
    for (width, want) in color["table"]["renders"].as_object().expect("map") {
        check.eq(&format!("styled table at {width}"), &table.render(width.parse().expect("width"), true), &s(want));
    }
    for spec in color["panels"].as_array().expect("list") {
        let body = match spec["markup"].as_str() {
            Some(markup) => Text::from_markup(markup),
            None => assembled(&spec["body"]),
        };
        let panel = Panel::new(body).title(&s(&spec["title"])).border_style(&s(&spec["border"]));
        for (width, want) in spec["renders"].as_object().expect("map") {
            let got = panel.render(width.parse().expect("width"), true);
            check.eq(&format!("styled panel {:?} at {width}", s(&spec["title"])), &got, &s(want));
        }
    }
    for spec in color["prints"].as_array().expect("list") {
        let text = Text::from_markup(&s(&spec["markup"]));
        for (width, want) in spec["wrapped"].as_object().expect("map") {
            let got = render_text(&text, width.parse().expect("width"), true);
            check.eq(&format!("styled print {:?} at {width}", s(&spec["markup"])), &got, &s(want));
        }
        check.eq(
            &format!("styled soft print {:?}", s(&spec["markup"])),
            &render_text_soft(&text, true),
            &s(&spec["soft_wrap"]),
        );
    }
    check.finish();
}
