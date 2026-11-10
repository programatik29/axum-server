use axum::{routing::get, Router};
use axum_server::{Control, Handle};
use std::{io, net::SocketAddr, time::Duration};
use tokio::task::JoinSet;

#[tokio::main]
async fn main() -> io::Result<()> {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    // One control handle drains every listener below.
    let control = Control::new();
    let mut servers = JoinSet::new();

    let tcp_handle = Handle::<SocketAddr>::with_control(control.clone());
    let tcp =
        axum_server::bind(SocketAddr::from(([127, 0, 0, 1], 3000))).handle(tcp_handle.clone());
    let tcp_service = app.clone().into_make_service();

    servers.spawn(async move { tcp.serve(tcp_service).await });

    #[cfg(unix)]
    let unix_handle = {
        use std::os::unix::net::SocketAddr;

        let path = std::env::temp_dir().join("axum-server-multiple-listeners.sock");
        let _ = std::fs::remove_file(&path);

        let handle = Handle::with_control(control.clone());
        let unix = axum_server::bind(SocketAddr::from_pathname(path)?).handle(handle.clone());
        let service = app.clone().into_make_service();

        servers.spawn(async move { unix.serve(service).await });

        handle
    };

    println!("tcp: http://{}/", tcp_handle.listening().await.unwrap());
    #[cfg(unix)]
    println!(
        "unix: {:?}",
        unix_handle
            .listening()
            .await
            .unwrap()
            .as_pathname()
            .unwrap()
    );

    let shutdown = control.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown.graceful_shutdown(Some(Duration::from_secs(10)));
    });

    // A failing listener stops the others instead of leaving them running.
    while let Some(result) = servers.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                eprintln!("server error: {error}");
                control.shutdown();
            }
            Err(error) => {
                eprintln!("server task failed: {error}");
                control.shutdown();
            }
        }
    }

    #[cfg(unix)]
    let _ = std::fs::remove_file(std::env::temp_dir().join("axum-server-multiple-listeners.sock"));

    Ok(())
}
