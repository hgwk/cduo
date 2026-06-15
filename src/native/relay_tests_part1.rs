use super::*;

#[test]
fn short_transcript_output_policy_has_explicit_boundaries() {
    assert!(should_suppress_transcript_output(""));
    assert!(should_suppress_transcript_output("OK"));
    assert!(should_suppress_transcript_output("Done."));
    assert!(should_suppress_transcript_output("123456"));
    assert!(!should_suppress_transcript_output("1234567"));
    assert!(!should_suppress_transcript_output("valid longer response"));
}
