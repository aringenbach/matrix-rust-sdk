use std::{collections::HashMap, option_env};

use anyhow::Result;
use assign::assign;
use matrix_sdk::{
    config::RequestConfig,
    ruma::api::client::{account::register::v3::Request as RegistrationRequest, uiaa},
    Client,
};
use once_cell::sync::Lazy;
use tempfile::{tempdir, TempDir};
use tokio::sync::Mutex;

static USERS: Lazy<Mutex<HashMap<String, (Client, TempDir)>>> = Lazy::new(Mutex::default);

#[ctor::ctor]
fn init_logging() {
    use tracing::Level;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(Level::TRACE.into())
                .from_env()
                .unwrap(),
        )
        .with(tracing_subscriber::fmt::layer().with_test_writer())
        .init();
}

/// read the test configuration from the environment
pub fn test_server_conf() -> (String, String) {
    (
        option_env!("HOMESERVER_URL").unwrap_or("http://localhost:8228").to_owned(),
        option_env!("HOMESERVER_DOMAIN").unwrap_or("matrix-sdk.rs").to_owned(),
    )
}

pub async fn get_client_for_user(username: String, use_sqlite_store: bool) -> Result<Client> {
    let mut users = USERS.lock().await;
    if let Some((client, _)) = users.get(&username) {
        return Ok(client.clone());
    }

    let (homeserver_url, _domain_name) = test_server_conf();

    let tmp_dir = tempdir()?;

    let client_builder = Client::builder()
        .user_agent("matrix-sdk-integration-tests")
        .homeserver_url(homeserver_url)
        .request_config(RequestConfig::short_retry());
    let client = if use_sqlite_store {
        client_builder.sqlite_store(tmp_dir.path(), None).build().await?
    } else {
        client_builder.build().await?
    };

    // safe to assume we have not registered this user yet, but ignore if we did

    if let Err(resp) = client.register(RegistrationRequest::new()).await {
        // FIXME: do actually check the registration types...
        if let Some(_response) = resp.as_uiaa_response() {
            let request = assign!(RegistrationRequest::new(), {
                username: Some(username.clone()),
                password: Some(username.clone()),

                auth: Some(uiaa::AuthData::Dummy(uiaa::Dummy::new())),
            });
            // we don't care if this failed, then we just try to login anyways
            let _ = client.register(request).await;
        }
    }
    client.login_username(&username, &username).await?;
    users.insert(username, (client.clone(), tmp_dir)); // keeping temp dir around so it doesn't get destroyed yet

    Ok(client)
}
