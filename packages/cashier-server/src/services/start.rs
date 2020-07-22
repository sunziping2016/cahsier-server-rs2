use crate::{
    api::{
        api_v1,
        app_state::{AppConfig, AppSubscriber, AppSmtp, AppDatabase},
    },
    config::StartConfig,
    websocket::main_subscriber::MainSubscriber,
};
use actix::Actor;
use actix_files as fs;
use actix_web::{
    web, App, HttpServer,
    middleware::Logger,
};
use err_derive::Error;
use lettre::{
    SmtpTransport,
    Tls, TlsParameters,
    transport::smtp::authentication::Credentials,
};
use log::error;
use redis::RedisError;
use rustls::ClientConfig;
use tokio_postgres::{
    Error as PostgresError,
    NoTls,
};
use webpki_roots::TLS_SERVER_ROOTS;

#[derive(Debug, Error)]
pub enum StartError {
    #[error(display = "{}", _0)]
    Db(#[error(source)] #[error(from)] PostgresError),
    #[error(display = "{}", _0)]
    Io(#[error(source)] #[error(from)] std::io::Error),
    #[error(display = "{}", _0)]
    Redis(#[error(source)] #[error(from)] RedisError),
}

pub type Result<T> = std::result::Result<T, StartError>;

pub async fn start(config: &StartConfig) -> Result<()> {
    let (client, connection) = tokio_postgres::connect(&config.db, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            error!("connection error: {}", e);
        }
    });
    let redis_client = redis::Client::open(&config.redis[..])?;
    let redis_connection = redis_client.get_async_connection().await?;
    let subscriber = MainSubscriber::new(
        redis_client.get_async_connection().await?,
        redis_connection.into_pubsub()
    ).start();
    let mut tls_config = ClientConfig::new();
    tls_config.root_store.add_server_trust_anchors(&TLS_SERVER_ROOTS);

    let mut smtp_builder = SmtpTransport::builder(config.smtp.server.clone());
    if let Some(ref username) = config.smtp.username {
        if let Some(ref password) = config.smtp.password {
            smtp_builder = smtp_builder.credentials(Credentials::new(
                username.clone(),
                password.clone(),
            ));
        }
    }

    let smtp = smtp_builder.tls(Tls::Required(TlsParameters::new(
        config.smtp.server.clone(),
        tls_config,
    )))
        .build();

    let app_config = web::Data::new(AppConfig {
        config: config.clone(),
    });
    let app_database = web::Data::new(AppDatabase::new(client).await);
    let app_subscriber = web::Data::new(AppSubscriber {
        subscriber,
    });
    let app_smtp = web::Data::new(AppSmtp {
        smtp,
    });
    let media_serve = config.media.serve;
    let media_url = config.media.url.clone();
    let media_root = config.media.root.clone();
    HttpServer::new(move || {
        let mut app = App::new()
            .wrap(Logger::default())
            .configure(api_v1(
                &app_config,
                &app_database,
                &app_subscriber,
                &app_smtp,
            ));
        if media_serve {
            app = app.service(fs::Files::new(&media_url, &media_root))
        }
        app
    })
        .bind(&config.bind)?
        .run()
        .await?;
    Ok(())
}