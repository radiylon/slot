mod common;

use common::{app_booting_at, tmp_root_with_carts, CLOCK_IS_SET};
use slot::app::{App, Phase, SWITCH_VOLUME_MAX};
use slot::bluetooth::{to_headphones, BtJob, Headphones, ScriptedBluetooth};
use slot::bt_menu::BtPage;
use slot_input::{Action, Btn};
use slot_store::{read_slot_state, write_slot_state, SlotState, VOLUME_BT_DEFAULT};
use slot_ui::QuickRow;
use tempfile::TempDir;

const PODS: &str = "74:77:86:15:3A:A5";
const OTHER: &str = "AA:BB:CC:DD:EE:02";

struct Rig {
    d: TempDir,
    a: App,
    bt: ScriptedBluetooth,
    t: u64,
}

impl Rig {
    fn new(state: SlotState) -> Self {
        let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
        write_slot_state(
            d.path(),
            &SlotState {
                clock_set: true,
                ..state
            },
        )
        .unwrap();
        let (mut a, _clock) = app_booting_at(d.path(), CLOCK_IS_SET);
        let bt = ScriptedBluetooth::with_nearby(&[
            (PODS, "Rad’s Airpods Pro 3 - Find My"),
            (OTHER, "Desk Speaker"),
        ]);
        a.set_bluetooth(Box::new(bt.clone()));
        let mut r = Rig { d, a, bt, t: 1_000 };
        r.tick(0);
        r
    }

    fn tick(&mut self, ms: u64) {
        self.t += ms;
        self.a.tick_ms(self.t);
    }

    fn press(&mut self, btn: Btn) {
        self.a.apply(Action::GbaDown(btn));
        self.a.apply(Action::GbaUp(btn));
        self.tick(16);
    }

    fn open_page(&mut self) {
        self.a.apply(Action::QuickMenu);
        for _ in 0..QuickRow::Audio.position(&[]) {
            self.press(Btn::Down);
        }
        assert_eq!(
            self.a.quick_menu(),
            Some(QuickRow::Audio),
            "phase {:?}",
            self.a.phase()
        );
        self.press(Btn::A);
        assert!(matches!(
            self.a.phase(),
            Phase::Audio {
                page: BtPage::Main,
                row: 0
            }
        ));
    }

    fn row_labels(&self) -> Vec<String> {
        self.a
            .bt_screen()
            .expect("on the Bluetooth page")
            .lines
            .into_iter()
            .map(|l| l.label)
            .collect()
    }

    fn select(&mut self, label: &str) {
        let at = self
            .row_labels()
            .iter()
            .position(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("no {label:?} row in {:?}", self.row_labels()));
        let Phase::Audio { row, .. } = *self.a.phase() else {
            panic!("left the Bluetooth page");
        };
        let n = self.row_labels().len();
        for _ in 0..(at + n - row) % n {
            self.press(Btn::Down);
        }
    }

    fn headphones_volume(&self) -> Option<u8> {
        self.bt.script().view.volume
    }

    fn saved(&self) -> SlotState {
        self.a.flush_state();
        read_slot_state(self.d.path())
    }
}

fn paired(rig: &Rig, mac: &str, name: &str) {
    rig.bt.script().view.paired.push(Headphones {
        mac: mac.into(),
        name: name.into(),
        connected: false,
    });
    rig.bt.script().changed();
}

fn using_pods() -> Rig {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        bt_device: Some(PODS.into()),
        ..SlotState::default()
    });
    paired(&r, PODS, "Pods");
    r.a.start_bluetooth();
    r.tick(16);
    r.tick(16);
    r
}

#[test]
fn the_switch_turns_the_radio_on_and_is_remembered() {
    let mut r = Rig::new(SlotState::default());
    r.open_page();
    assert_eq!(
        r.row_labels(),
        ["Speaker", "Bluetooth"],
        "a dead radio offered more than its switch"
    );
    r.select("Bluetooth");
    r.press(Btn::A);
    assert!(r.bt.asked().contains(&BtJob::Up));
    assert!(r.saved().bluetooth);
    assert_eq!(r.row_labels(), ["Speaker", "Bluetooth", "Pair Headphones"]);
    r.press(Btn::A);
    assert!(r.bt.asked().contains(&BtJob::Down));
    assert!(!r.saved().bluetooth);
}

#[test]
fn pairing_found_headphones_connects_them_and_makes_them_the_output() {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        ..SlotState::default()
    });
    r.open_page();
    r.select("Pair Headphones");
    r.press(Btn::A);
    assert!(matches!(
        r.a.phase(),
        Phase::Audio {
            page: BtPage::Pair,
            ..
        }
    ));
    assert!(r.bt.asked().contains(&BtJob::Scan));
    r.tick(16);
    assert_eq!(
        r.row_labels(),
        ["Rad's Airpods Pro 3...", "Desk Speaker", "Search Again"]
    );
    r.press(Btn::A);
    r.tick(16);
    assert!(r.bt.asked().contains(&BtJob::Pair(PODS.into())));
    assert!(
        matches!(
            r.a.phase(),
            Phase::Audio {
                page: BtPage::Main,
                row: 1
            }
        ),
        "pairing did not land on the new pair: {:?}",
        r.a.phase()
    );
    assert_eq!(r.saved().bt_device.as_deref(), Some(PODS));
    assert!(r.a.bt_routed());
    let line = &r.a.bt_screen().unwrap().lines[1];
    assert_eq!(line.value.as_deref(), Some("Connected"));
}

#[test]
fn a_new_link_starts_at_slots_quiet_level_not_the_headphones_own() {
    let r = using_pods();
    assert_eq!(
        r.headphones_volume(),
        Some(to_headphones(VOLUME_BT_DEFAULT)),
        "the headphones kept the level the last phone left them at"
    );
}

#[test]
fn the_volume_keys_turn_the_headphones_down_and_leave_the_samples_whole() {
    let mut r = using_pods();
    let before = r.headphones_volume().unwrap();
    r.a.apply(Action::VolumeDown);
    r.tick(16);
    let after = r.headphones_volume().unwrap();
    assert!(after < before, "volume down left the headphones at {after}");
    assert_eq!(r.a.sample_volume(), 100, "the samples were scaled twice");
    assert_eq!(r.saved().volume_bt, VOLUME_BT_DEFAULT - 5);
    assert_eq!(
        r.saved().volume,
        SlotState::default().volume,
        "the speaker level moved"
    );
}

#[test]
fn mute_silences_the_headphones() {
    let mut r = using_pods();
    r.a.apply(Action::MuteToggle);
    r.tick(16);
    assert_eq!(r.headphones_volume(), Some(0));
    assert_eq!(r.a.sample_volume(), 0);
}

#[test]
fn a_swipe_on_the_headphones_becomes_slots_level() {
    let mut r = using_pods();
    r.tick(3_000);
    r.bt.script().view.volume = Some(to_headphones(80));
    r.bt.script().changed();
    r.tick(16);
    assert_eq!(r.a.volume(), 80);
    assert_eq!(r.saved().volume_bt, 80);
}

#[test]
fn a_stale_reading_just_after_a_key_press_does_not_undo_it() {
    let mut r = using_pods();
    r.tick(3_000);
    r.a.apply(Action::VolumeUp);
    r.tick(16);
    let set = r.a.volume();
    r.bt.script().view.volume = Some(to_headphones(VOLUME_BT_DEFAULT));
    r.bt.script().changed();
    r.tick(16);
    assert_eq!(r.a.volume(), set, "a reading from before the press won");
}

#[test]
fn headphones_out_of_reach_keep_the_sound_off_the_speaker_and_are_retried() {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        bt_device: Some(PODS.into()),
        ..SlotState::default()
    });
    paired(&r, PODS, "Pods");
    r.bt.script().unreachable = true;
    r.a.start_bluetooth();
    r.tick(16);
    assert!(
        r.a.bt_routed(),
        "the speaker took over while the headphones were away"
    );
    let connects = |r: &Rig| {
        r.bt.asked()
            .iter()
            .filter(|j| **j == BtJob::Connect(PODS.into()))
            .count()
    };
    let first = connects(&r);
    r.tick(16_000);
    assert!(connects(&r) > first, "slot gave up on the headphones");
    r.bt.script().unreachable = false;
    r.tick(16_000);
    r.tick(16);
    assert_eq!(
        r.headphones_volume(),
        Some(to_headphones(VOLUME_BT_DEFAULT))
    );
}

#[test]
fn choosing_the_speaker_hands_the_sound_back() {
    let mut r = using_pods();
    r.open_page();
    r.select("Speaker");
    r.press(Btn::A);
    assert!(r.bt.asked().contains(&BtJob::Speaker));
    assert!(!r.a.bt_routed());
    assert_eq!(r.saved().bt_device, None);
    assert_eq!(
        r.a.volume(),
        SlotState::default().volume.min(SWITCH_VOLUME_MAX),
        "the speaker kept the headphones' level"
    );
}

#[test]
fn forgetting_headphones_takes_a_second_press() {
    let mut r = using_pods();
    r.open_page();
    r.select("Pods");
    r.press(Btn::Y);
    assert_eq!(
        r.a.bt_screen().unwrap().status.as_deref(),
        Some("Press Y again to forget")
    );
    assert!(!r.bt.asked().contains(&BtJob::Forget(PODS.into())));
    r.press(Btn::Y);
    assert!(r.bt.asked().contains(&BtJob::Forget(PODS.into())));
    assert_eq!(r.saved().bt_device, None);
    r.tick(16);
    assert_eq!(r.row_labels(), ["Speaker", "Bluetooth", "Pair Headphones"]);
}

#[test]
fn a_refused_pairing_says_so_and_leaves_the_output_alone() {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        ..SlotState::default()
    });
    r.bt.script().refuse_pairing = true;
    r.open_page();
    r.select("Pair Headphones");
    r.press(Btn::A);
    r.tick(16);
    r.press(Btn::A);
    r.tick(16);
    assert_eq!(
        r.a.bt_screen().unwrap().status.as_deref(),
        Some("Could not pair")
    );
    assert!(matches!(
        r.a.phase(),
        Phase::Audio {
            page: BtPage::Pair,
            ..
        }
    ));
    assert_eq!(r.saved().bt_device, None);
}

#[test]
fn b_backs_out_a_page_at_a_time() {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        ..SlotState::default()
    });
    r.open_page();
    r.select("Pair Headphones");
    r.press(Btn::A);
    r.press(Btn::B);
    assert!(matches!(
        r.a.phase(),
        Phase::Audio {
            page: BtPage::Main,
            ..
        }
    ));
    r.press(Btn::B);
    assert_eq!(r.a.quick_menu(), Some(QuickRow::Audio));
}

#[test]
fn booting_with_bluetooth_on_reconnects_the_chosen_headphones() {
    let r = using_pods();
    let asked = r.bt.asked();
    let up = asked
        .iter()
        .position(|j| *j == BtJob::Up)
        .expect("never turned the radio on");
    let connect = asked
        .iter()
        .position(|j| *j == BtJob::Connect(PODS.into()))
        .expect("never reconnected");
    assert!(up < connect);
}

#[test]
fn moving_to_the_headphones_never_starts_them_loud() {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        volume: 100,
        volume_bt: 100,
        ..SlotState::default()
    });
    paired(&r, PODS, "Pods");
    r.tick(16);
    r.open_page();
    r.select("Pods");
    r.press(Btn::A);
    r.tick(16);
    assert!(
        r.a.volume() <= SWITCH_VOLUME_MAX,
        "the headphones came on at {}",
        r.a.volume()
    );
    assert!(r.headphones_volume().unwrap() <= to_headphones(SWITCH_VOLUME_MAX));
}

#[test]
fn moving_back_to_the_speaker_never_starts_it_loud() {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        bt_device: Some(PODS.into()),
        volume: 100,
        ..SlotState::default()
    });
    paired(&r, PODS, "Pods");
    r.a.start_bluetooth();
    r.tick(16);
    r.open_page();
    r.select("Speaker");
    r.press(Btn::A);
    assert!(!r.a.bt_routed());
    assert!(
        r.a.volume() <= SWITCH_VOLUME_MAX,
        "the speaker came back at {}",
        r.a.volume()
    );
}

#[test]
fn turning_bluetooth_off_under_the_headphones_brings_the_speaker_back_quietly() {
    let mut r = Rig::new(SlotState {
        bluetooth: true,
        bt_device: Some(PODS.into()),
        volume: 90,
        ..SlotState::default()
    });
    paired(&r, PODS, "Pods");
    r.a.start_bluetooth();
    r.tick(16);
    r.open_page();
    r.select("Bluetooth");
    r.press(Btn::A);
    assert!(!r.a.bt_routed());
    assert!(r.a.volume() <= SWITCH_VOLUME_MAX);
}

impl Rig {
    fn play(&mut self) {
        self.a.apply(Action::Insert);
        self.a.on_core_ready();
        for _ in 0..600 {
            if matches!(self.a.phase(), Phase::Playing { .. }) {
                // Let the power-on finish, so the screen is fully up.
                for _ in 0..120 {
                    self.a.update(1.0 / 60.0);
                    self.tick(16);
                }
                return;
            }
            self.a.update(1.0 / 60.0);
            self.tick(16);
        }
        panic!("the cart never seated: {:?}", self.a.phase());
    }

    fn open_page_in_game(&mut self) {
        self.a.apply(Action::QuickMenu);
        assert_eq!(self.a.in_game_menu(), Some(QuickRow::Resume));
        for _ in 0..QuickRow::Settings.position(self.a.in_game_hidden()) {
            self.press(Btn::Down);
        }
        self.press(Btn::A);
        for _ in 0..QuickRow::PlayAudio.position(self.a.in_game_hidden()) {
            self.press(Btn::Down);
        }
        assert_eq!(self.a.in_game_menu(), Some(QuickRow::PlayAudio));
        assert!(self.a.bt_screen().is_none(), "Audio opened before A");
        self.press(Btn::A);
        assert!(self.a.bt_screen().is_some(), "A did not open Audio");
    }

    fn down_to(&mut self, label: &str) {
        let at = self
            .row_labels()
            .iter()
            .position(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("no {label:?} row in {:?}", self.row_labels()));
        for _ in 0..at {
            self.press(Btn::Down);
        }
    }
}

#[test]
fn audio_opens_over_the_paused_game_and_b_backs_out_to_its_settings() {
    let mut r = using_pods();
    r.play();
    r.open_page_in_game();
    assert!(
        matches!(r.a.phase(), Phase::Playing { .. }),
        "the game was left"
    );
    let mut out = Vec::new();
    r.a.draw(&mut out);
    assert!(
        out.iter()
            .any(|d| matches!(*d, slot_ui::Draw::Rect { colour, .. }
            if colour[3] > 0.0 && colour[3] < 1.0)),
        "the game is not dimmed behind the Audio page"
    );
    r.press(Btn::B);
    assert!(r.a.bt_screen().is_none());
    assert_eq!(r.a.in_game_menu(), Some(QuickRow::PlayAudio));
    r.press(Btn::B);
    assert_eq!(r.a.in_game_menu(), Some(QuickRow::Settings));
}

#[test]
fn the_speaker_can_be_chosen_mid_game() {
    let mut r = using_pods();
    r.play();
    assert!(r.a.bt_routed());
    r.open_page_in_game();
    r.down_to("Speaker");
    r.press(Btn::A);
    assert!(!r.a.bt_routed());
    assert_eq!(r.saved().bt_device, None);
    assert!(matches!(r.a.phase(), Phase::Playing { .. }));
}

#[test]
fn a_tap_of_menu_on_the_in_game_audio_page_resumes_the_game() {
    let mut r = using_pods();
    r.play();
    r.open_page_in_game();
    r.a.apply(Action::QuickMenu);
    r.tick(16);
    assert_eq!(r.a.in_game_menu(), None);
    assert!(r.a.bt_screen().is_none());
    r.a.apply(Action::QuickMenu);
    assert_eq!(r.a.in_game_menu(), Some(QuickRow::Resume));
    assert!(r.a.bt_screen().is_none(), "Audio came back on its own");
}
