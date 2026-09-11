fn main() {
    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    unsafe extern "C" {
        fn adapter_monotonic_clock_set_paused(paused: bool);
    }

    unsafe {
        assert!(wasip1::clock_time_get(wasip1::CLOCKID_REALTIME, 0).unwrap() > 0);
        let monotonic = wasip1::clock_time_get(wasip1::CLOCKID_MONOTONIC, 0).unwrap();
        let realtime = wasip1::clock_time_get(wasip1::CLOCKID_REALTIME, 0).unwrap();
        adapter_monotonic_clock_set_paused(true);
        assert_eq!(
            wasip1::clock_time_get(wasip1::CLOCKID_REALTIME, 0).unwrap(),
            realtime,
        );
        assert_eq!(
            wasip1::clock_time_get(wasip1::CLOCKID_MONOTONIC, 0).unwrap(),
            monotonic,
        );
        adapter_monotonic_clock_set_paused(false);
        assert!(wasip1::clock_time_get(wasip1::CLOCKID_REALTIME, 0).unwrap() > realtime);
        assert!(wasip1::clock_time_get(wasip1::CLOCKID_MONOTONIC, 0).unwrap() > monotonic);
    }
}
