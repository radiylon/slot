//! What the Audio page says: where the sound goes, the Bluetooth switch, pairing, the
//! status line and the hints.

use slot_ui::BT_ROWS_MAX;

use crate::bluetooth::{shown_name, BtTask, BtView};

const NAME_CHARS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtPage {
    Main,
    Pair,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BtRow {
    Power,
    Device(String),
    Speaker,
    PairNew,
    Found(String),
    Rescan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtLine {
    pub label: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtScreen {
    pub lines: Vec<BtLine>,
    pub row: usize,
    pub status: Option<String>,
    pub legend: Vec<(&'static str, &'static str)>,
}

pub struct BtState<'a> {
    pub view: &'a BtView,
    pub on: bool,
    pub chosen: Option<&'a str>,
    /// Wired headphones are in, so the built-in output is the jack, not the speaker.
    pub jack: bool,
    pub forget_armed: Option<&'a str>,
}

pub fn rows(page: BtPage, s: &BtState) -> Vec<BtRow> {
    match page {
        BtPage::Main if !s.on => vec![BtRow::Speaker, BtRow::Power],
        BtPage::Main => std::iter::once(BtRow::Speaker)
            .chain(
                s.view
                    .paired
                    .iter()
                    .take(BT_ROWS_MAX - 3)
                    .map(|h| BtRow::Device(h.mac.clone())),
            )
            .chain([BtRow::Power, BtRow::PairNew])
            .collect(),
        BtPage::Pair => s
            .view
            .found
            .iter()
            .take(BT_ROWS_MAX - 1)
            .map(|h| BtRow::Found(h.mac.clone()))
            .chain(std::iter::once(BtRow::Rescan))
            .collect(),
    }
}

fn name(view: &BtView, mac: &str) -> String {
    let name = view
        .paired
        .iter()
        .chain(&view.found)
        .find(|h| h.mac == mac)
        .map_or(mac, |h| h.name.as_str());
    let shown = shown_name(name);
    if shown.chars().count() <= NAME_CHARS {
        return shown;
    }
    let cut: String = shown.chars().take(NAME_CHARS - 3).collect();
    let whole = match cut.rfind(' ') {
        Some(at) if at > 0 => &cut[..at],
        _ => cut.as_str(),
    };
    format!("{}...", whole.trim_end_matches([' ', '-']))
}

fn scanning(view: &BtView) -> bool {
    matches!(view.busy, Some((BtTask::Scanning, _)))
}

fn line(row: &BtRow, s: &BtState) -> BtLine {
    let v = s.view;
    let (label, value): (String, Option<&str>) = match row {
        BtRow::Power => (
            "Bluetooth".into(),
            Some(match (&v.busy, s.on) {
                (Some((BtTask::Starting, _)), _) => "Starting",
                (Some((BtTask::Stopping, _)), _) => "Stopping",
                (_, true) => "On",
                (_, false) => "Off",
            }),
        ),
        BtRow::Device(mac) => (
            name(v, mac),
            match v.busy_with(mac) {
                Some(BtTask::Connecting) => Some("Connecting"),
                Some(BtTask::Forgetting) => Some("Forgetting"),
                _ if s.chosen == Some(mac) && v.connected(mac) => Some("Connected"),
                _ if s.chosen == Some(mac) => Some("Waiting"),
                _ => None,
            },
        ),
        BtRow::Speaker => (
            if s.jack { "Headphone Jack" } else { "Speaker" }.into(),
            s.chosen.is_none().then_some("In Use"),
        ),
        BtRow::PairNew => ("Pair Headphones".into(), None),
        BtRow::Found(mac) => (
            name(v, mac),
            (v.busy_with(mac) == Some(BtTask::Pairing)).then_some("Pairing"),
        ),
        BtRow::Rescan => ("Search Again".into(), scanning(v).then_some("Searching")),
    };
    BtLine {
        label,
        value: value.map(str::to_string),
    }
}

fn status(page: BtPage, s: &BtState) -> Option<String> {
    if s.forget_armed.is_some() {
        return Some("Press Y again to forget".into());
    }
    if let Some(why) = &s.view.failed {
        return Some(why.clone());
    }
    match page {
        BtPage::Pair if scanning(s.view) => Some("Put headphones in pairing mode".into()),
        BtPage::Pair if s.view.found.is_empty() => Some("No headphones found".into()),
        _ => None,
    }
}

fn legend(row: Option<&BtRow>, s: &BtState) -> Vec<(&'static str, &'static str)> {
    let mut hints = vec![("B", "Back")];
    let act = match row {
        Some(BtRow::Power) if s.on => Some("Turn Off"),
        Some(BtRow::Power) => Some("Turn On"),
        Some(BtRow::Device(mac)) if s.chosen == Some(mac) && s.view.connected(mac) => None,
        Some(BtRow::Device(_)) => Some("Connect"),
        Some(BtRow::Speaker) if s.chosen.is_some() => Some("Use"),
        Some(BtRow::Speaker) => None,
        Some(BtRow::PairNew) => Some("Open"),
        Some(BtRow::Found(_)) => Some("Pair"),
        Some(BtRow::Rescan) => Some("Search"),
        None => None,
    };
    if let Some(act) = act {
        hints.push(("A", act));
    }
    if let Some(BtRow::Device(_)) = row {
        hints.push(("Y", "Forget"));
    }
    hints
}

pub fn screen(page: BtPage, row: usize, s: &BtState) -> BtScreen {
    let rows = rows(page, s);
    let row = row.min(rows.len().saturating_sub(1));
    BtScreen {
        lines: rows.iter().map(|r| line(r, s)).collect(),
        row,
        status: status(page, s),
        legend: legend(rows.get(row), s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth::Headphones;

    fn pods(connected: bool) -> Headphones {
        Headphones {
            mac: "74:77:86:15:3A:A5".into(),
            name: "Rad’s Airpods Pro 3 - Find My".into(),
            connected,
        }
    }

    #[test]
    fn with_the_radio_off_the_page_is_the_speaker_and_a_switch() {
        let view = BtView::default();
        let s = BtState {
            view: &view,
            on: false,
            chosen: None,
            jack: false,
            forget_armed: None,
        };
        assert_eq!(rows(BtPage::Main, &s), vec![BtRow::Speaker, BtRow::Power]);
        assert_eq!(
            screen(BtPage::Main, 1, &s).legend,
            vec![("B", "Back"), ("A", "Turn On")]
        );
        assert_eq!(
            screen(BtPage::Main, 0, &s).lines[0].value.as_deref(),
            Some("In Use")
        );
    }

    #[test]
    fn the_headphones_in_use_say_so_and_the_speaker_steps_aside() {
        let view = BtView {
            powered: true,
            paired: vec![pods(true)],
            ..BtView::default()
        };
        let s = BtState {
            view: &view,
            on: true,
            chosen: Some("74:77:86:15:3A:A5"),
            jack: false,
            forget_armed: None,
        };
        let sc = screen(BtPage::Main, 1, &s);
        assert_eq!(sc.lines[1].label, "Rad's Airpods Pro 3...");
        assert_eq!(sc.lines[1].value.as_deref(), Some("Connected"));
        assert_eq!(sc.lines[0].value, None, "the speaker claims to be in use");
        assert_eq!(
            rows(BtPage::Main, &s),
            vec![
                BtRow::Speaker,
                BtRow::Device("74:77:86:15:3A:A5".into()),
                BtRow::Power,
                BtRow::PairNew
            ],
            "the outputs do not come first"
        );
        assert_eq!(sc.legend, vec![("B", "Back"), ("Y", "Forget")]);
    }

    #[test]
    fn chosen_headphones_out_of_reach_are_waited_for() {
        let view = BtView {
            powered: true,
            paired: vec![pods(false)],
            ..BtView::default()
        };
        let s = BtState {
            view: &view,
            on: true,
            chosen: Some("74:77:86:15:3A:A5"),
            jack: false,
            forget_armed: None,
        };
        assert_eq!(
            screen(BtPage::Main, 1, &s).lines[1].value.as_deref(),
            Some("Waiting")
        );
    }

    #[test]
    fn a_failure_is_the_status_line() {
        let view = BtView {
            powered: true,
            failed: Some("Could not connect".into()),
            ..BtView::default()
        };
        let s = BtState {
            view: &view,
            on: true,
            chosen: None,
            jack: false,
            forget_armed: None,
        };
        assert_eq!(
            screen(BtPage::Main, 0, &s).status.as_deref(),
            Some("Could not connect")
        );
    }

    #[test]
    fn the_pair_page_lists_what_the_scan_found_and_can_search_again() {
        let view = BtView {
            powered: true,
            found: vec![pods(false)],
            ..BtView::default()
        };
        let s = BtState {
            view: &view,
            on: true,
            chosen: None,
            jack: false,
            forget_armed: None,
        };
        assert_eq!(
            rows(BtPage::Pair, &s),
            vec![BtRow::Found("74:77:86:15:3A:A5".into()), BtRow::Rescan]
        );
        assert_eq!(screen(BtPage::Pair, 0, &s).legend[1], ("A", "Pair"));
    }

    #[test]
    fn a_long_list_never_runs_past_the_hints() {
        let paired: Vec<Headphones> = (0..9)
            .map(|i| Headphones {
                mac: format!("AA:BB:CC:DD:EE:0{i}"),
                name: format!("Pair {i}"),
                connected: false,
            })
            .collect();
        let view = BtView {
            powered: true,
            paired: paired.clone(),
            found: paired,
            ..BtView::default()
        };
        let s = BtState {
            view: &view,
            on: true,
            chosen: None,
            jack: false,
            forget_armed: None,
        };
        assert!(rows(BtPage::Main, &s).len() <= BT_ROWS_MAX);
        assert!(rows(BtPage::Pair, &s).len() <= BT_ROWS_MAX);
    }

    #[test]
    fn with_wired_headphones_in_the_built_in_output_is_the_jack() {
        let view = BtView::default();
        let s = BtState {
            view: &view,
            on: false,
            chosen: None,
            jack: true,
            forget_armed: None,
        };
        assert_eq!(screen(BtPage::Main, 0, &s).lines[0].label, "Headphone Jack");
    }
}
