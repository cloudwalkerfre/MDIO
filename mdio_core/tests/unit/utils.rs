use super::*;

#[test]
fn test_measure() {
    let (dur, result) = measure(|| {
        std::thread::sleep(Duration::from_millis(10));
        42
    });

    assert_eq!(result, 42);
    assert!(dur.as_millis() >= 10);
}

#[test]
fn test_format_duration() {
    assert_eq!(format_duration(Duration::from_nanos(500)), "500ns");
    assert_eq!(format_duration(Duration::from_micros(10)), "10.000μs");
    assert_eq!(format_duration(Duration::from_millis(10)), "10.000ms");
    assert_eq!(format_duration(Duration::from_secs(1)), "1.000s");
}
