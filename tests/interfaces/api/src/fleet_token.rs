use dusk_connection::Connection;
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};

#[tokio::test(flavor = "current_thread")]
async fn test_dusk_answers_the_fleet_token_the_node_was_built_with() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let reply = client.fleet_token_request().send().promise.await.unwrap();
            let token = reply
                .get()
                .unwrap()
                .get_result()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            assert!(!token.is_empty(), "a node always carries a fleet token");
            assert_eq!(
                token,
                dusk_core::fleet_token::fleet_token(),
                "Dusk.fleetToken answers the token dusk_core was built with"
            );
            if option_env!("DUSK_FLEET_TOKEN").is_none() {
                assert!(
                    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "a token made up by the build is 64 hex digits: {} characters",
                    token.len()
                );
            }

            connection.disconnect().await.unwrap();
        })
        .await;
}
