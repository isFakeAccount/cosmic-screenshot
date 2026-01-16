use ashpd::desktop::screenshot::Screenshot;
use clap::{ArgAction, Parser};
use std::{
    collections::HashMap,
    fs::{self},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use zbus::{Connection, proxy, zvariant::Value};

mod localize;

#[derive(Parser, Default, Debug, Clone, PartialEq, Eq)]
#[command(version, about, long_about = None)]
struct Args {
    /// Enable interactive mode in the portal
    #[clap(long,
        default_missing_value("true"),
        default_value("true"),
        num_args(0..=1),
        require_equals(true),
        action = ArgAction::Set)]
    interactive: bool,
    /// Enable modal mode in the portal
    #[clap(long,
        default_missing_value("true"),
        default_value("true"),
        num_args(0..=1),
        require_equals(true),
        action = ArgAction::Set,)]
    modal: bool,
    /// Send a notification with the path to the saved screenshot
    #[clap(long,
        default_missing_value("true"),
        default_value("true"),
        num_args(0..=1),
        require_equals(true),
        action = ArgAction::Set)]
    notify: bool,
    /// The directory to save the screenshot to, if not performing an interactive screenshot
    #[clap(short, long)]
    save_dir: Option<PathBuf>,
}

#[proxy(assume_defaults = true)]
trait Notifications {
    /// Call the org.freedesktop.Notifications.Notify D-Bus method
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, &Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

fn move_picture(src_file: &Path, dst_file: &Path) {
    let src_meta = fs::metadata(src_file)
        .expect("Failed to get metadata on filesystem for picture source path.");

    let dst_dir = dst_file
        .parent()
        .expect("Failed to get parent directory of destination path.");
    let dst_meta = fs::metadata(dst_dir)
        .expect("Failed to get metadata on filesystem for picture destination.");

    if src_meta.dev() != dst_meta.dev() {
        fs::rename(src_file, dst_file).expect("Failed to move screenshot.");
        return;
    }

    fs::copy(src_file, dst_file).expect("Failed to move screenshot.");
    fs::remove_file(src_file).expect("Failed to remove temporary screenshot.");
}

//TODO: better error handling
#[tokio::main(flavor = "current_thread")]
async fn main() {
    crate::localize::localize();

    let args = Args::parse();
    let save_dir = (!args.interactive).then(|| {
        args.save_dir.filter(|dir| dir.is_dir()).unwrap_or_else(|| {
            let screenshot_dir = dirs::picture_dir().expect("failed to locate picture directory").join("Screenshots");
            fs::create_dir_all(&screenshot_dir).expect("Failed to create Screenshots dir.");
            screenshot_dir
        })
    });

    let response = Screenshot::request()
        .interactive(args.interactive)
        .modal(args.modal)
        .send()
        .await
        .expect("failed to send screenshot request")
        .response();

    let response = match response {
        Err(err) => {
            if err.to_string().contains("Cancelled") {
                println!("Screenshot cancelled by user");
                std::process::exit(0);
            }
            eprintln!("Error taking screenshot: {}", err);
            std::process::exit(1);
        }
        Ok(response) => response,
    };

    let uri = response.uri();
    let path = match uri.scheme() {
        "file" => {
            let response_path = uri
                .to_file_path()
                .unwrap_or_else(|()| panic!("unsupported response URI '{uri}'"));

            let date = chrono::Local::now();
            let filename = format!("Screenshot_{}.png", date.format("%Y-%m-%d_%H-%M-%S"));

            let pictures_dir = dirs::picture_dir().expect("Failed to locate Pictures directory.");
            let documents_dir =
                dirs::document_dir().expect("Failed to locate Documents directory.");

            let target_dir = if let Some(save_dir) = save_dir {
                save_dir
            } else if response_path.starts_with(&pictures_dir) {
                dirs::picture_dir()
                    .expect("Failed to locate picture directory.")
                    .join("Screenshots")
            } else if response_path.starts_with(&documents_dir) {
                dirs::document_dir().expect("Failed to locate document directory.")
            } else {
                response_path.clone()
            };

            fs::create_dir_all(&target_dir).unwrap_or_else(|_| {
                panic!("Failed to create directory '{}'", target_dir.display())
            });
            let target_img_path = target_dir.join(filename);
            move_picture(&response_path, &target_img_path);
            target_img_path.to_string_lossy().to_string()
        }
        "clipboard" => String::new(),
        scheme => panic!("unsupported scheme '{scheme}'"),
    };

    println!("{path}");

    if args.notify {
        let connection = Connection::session()
            .await
            .expect("failed to connect to session bus");

        let message = if path.is_empty() {
            fl!("screenshot-saved-to-clipboard")
        } else {
            fl!("screenshot-saved-to")
        };
        let proxy = NotificationsProxy::new(&connection)
            .await
            .expect("failed to create proxy");
        _ = proxy
            .notify(
                &fl!("cosmic-screenshot"),
                0,
                "com.system76.CosmicScreenshot",
                &message,
                &path,
                &[],
                HashMap::from([("transient", &Value::Bool(true))]),
                5000,
            )
            .await
            .expect("failed to send notification");
    }
}
