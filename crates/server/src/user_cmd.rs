//! `hulahoop user …`: the headless way to set a server up, for a host where
//! nobody can open the setup link in a browser.

use crate::{auth, db};
use anyhow::{bail, Context};
use std::path::PathBuf;

const USAGE: &str = "usage: hulahoop user create --owner --name NAME --email EMAIL \
     [--password PASSWORD | HULAHOOP_PASSWORD=…] [--data-dir DIR]\n       \
     hulahoop user setup-link [--data-dir DIR]";

#[derive(Default)]
struct Options {
    data_dir: Option<PathBuf>,
    owner: bool,
    name: Option<String>,
    email: Option<String>,
    password: Option<String>,
}

fn parse(args: &[String]) -> anyhow::Result<Options> {
    let mut options = Options {
        password: std::env::var("HULAHOOP_PASSWORD").ok(),
        ..Options::default()
    };
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = |what: &str| {
            args.next()
                .cloned()
                .with_context(|| format!("{flag} needs {what}"))
        };
        match flag.as_str() {
            "--owner" => options.owner = true,
            "--data-dir" => options.data_dir = Some(PathBuf::from(value("a path")?)),
            "--name" => options.name = Some(value("a name")?),
            "--email" => options.email = Some(value("an email")?),
            "--password" => options.password = Some(value("a password")?),
            other => bail!("unknown argument {other}\n{USAGE}"),
        }
    }
    Ok(options)
}

pub async fn run(args: Vec<String>) -> anyhow::Result<()> {
    let Some((command, rest)) = args.split_first() else {
        bail!("{USAGE}");
    };
    let options = parse(rest)?;
    let data_dir = options.data_dir.clone().unwrap_or_else(|| {
        std::env::var_os("HULAHOOP_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(crate::default_data_dir)
    });
    let database = db::Db::open(&data_dir).await?;
    database.seed_local().await?;
    match command.as_str() {
        "create" => create(&database, options).await,
        "setup-link" => setup_link(&database, &data_dir).await,
        other => bail!("unknown user command {other}\n{USAGE}"),
    }
}

async fn create(database: &db::Db, options: Options) -> anyhow::Result<()> {
    if !options.owner {
        bail!("only the first owner can be created here; invite others from Settings → Members\n{USAGE}");
    }
    let (Some(name), Some(email), Some(password)) = (options.name, options.email, options.password)
    else {
        bail!("--name, --email and a password (--password or HULAHOOP_PASSWORD) are required\n{USAGE}");
    };
    match auth::create_owner(database, &name, &email, &password).await {
        Ok(()) => {
            println!("{email} is now the owner; sign in from the Hulahoop app");
            Ok(())
        }
        Err(error) => bail!("{}", auth::describe(&error)),
    }
}

/// A fresh single-use link, for a server whose first link expired.
async fn setup_link(database: &db::Db, data_dir: &std::path::Path) -> anyhow::Result<()> {
    if !auth::needs_setup(database).await? {
        bail!("this server already has an owner; there is nothing to set up");
    }
    let token = auth::issue_setup_token(database).await?;
    let base = std::fs::read(data_dir.join("connection.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|connection| connection["url"].as_str().map(str::to_string))
        .unwrap_or_else(|| "http://<server address>".into());
    println!("{base}/setup?token={token}  (valid 30 minutes, one use)");
    Ok(())
}
