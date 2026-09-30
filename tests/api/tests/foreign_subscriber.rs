use dusk_program_logs::{Launcher, LogsConfig};
use dusk_tests as _;

#[test]
fn test_the_logs_launcher_refuses_a_process_whose_global_subscriber_is_taken() {
    tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default())
        .expect("install another global subscriber");
    let Err(error) = Launcher::new(LogsConfig::default()) else {
        panic!("the logs launcher started over another global subscriber");
    };
    assert!(
        error
            .to_string()
            .contains("couldn't install the logs subscriber"),
        "unexpected error: {error:#}"
    );
}
