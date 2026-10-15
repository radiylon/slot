use slot_store::{read_slot_state, write_slot_state, SlotState, VOLUME_BT_DEFAULT};

const _HEADPHONES_START_QUIET: () = assert!(VOLUME_BT_DEFAULT <= 50);

#[test]
fn the_headphones_and_their_level_round_trip() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Config")).unwrap();
    let s = SlotState {
        bluetooth: true,
        bt_device: Some("74:77:86:15:3A:A5".into()),
        volume_bt: 55,
        muted_bt: true,
        volume: 70,
        ..Default::default()
    };
    write_slot_state(d.path(), &s).unwrap();
    let back = read_slot_state(d.path());
    assert!(back.bluetooth);
    assert_eq!(back.bt_device.as_deref(), Some("74:77:86:15:3A:A5"));
    assert_eq!((back.volume_bt, back.muted_bt, back.volume), (55, true, 70));
}

#[test]
fn no_headphones_chosen_round_trips_as_none() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Config")).unwrap();
    write_slot_state(d.path(), &SlotState::default()).unwrap();
    let back = read_slot_state(d.path());
    assert_eq!((back.bluetooth, back.bt_device), (false, None));
}

#[test]
fn a_state_from_before_bluetooth_starts_with_it_off_and_quiet() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Config")).unwrap();
    std::fs::write(
        d.path().join("Config/slot.state"),
        "cart=\ncart_platform=\nbrightness=5\nblue_light=0\nvolume=90\nmuted=0\nclock_set=1\nutc_offset_min=0\n",
    )
    .unwrap();
    let s = read_slot_state(d.path());
    assert!(!s.bluetooth);
    assert_eq!(s.bt_device, None);
    assert_eq!(s.volume_bt, VOLUME_BT_DEFAULT);
}
