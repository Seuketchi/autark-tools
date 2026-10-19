//! `autark-accounts`: the accounts service (`org.autark.Accounts1`), and a small command line for it:
//! `autark-accounts list`, `add-google`, `add <provider> <email> [json]` (the password from
//! `AUTARK_ACCOUNT_PASSWORD`, else the first line on standard input; `json` overrides servers: `{"caldav":"https://…"}`), `remove <id>`.

use std::io::BufRead;

use autark_accounts::client::AccountsProxy;
use autark_accounts::{model, server};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.is_empty() || args == ["daemon"] { serve() } else { async_io::block_on(command(&args)) };
    if let Err(err) = result {
        eprintln!("accounts: {err}");
        std::process::exit(1);
    }
}

fn serve() -> Result<(), String> {
    async_io::block_on(async {
        let _connection = zbus::connection::Builder::session()
            .name("org.autark.Accounts1")
            .serve_at(server::PATH, server::Accounts::new())
            .build()
            .await
            .map_err(|err| err.to_string())?;
        std::future::pending::<()>().await;
        Ok(())
    })
}

async fn command(args: &[String]) -> Result<(), String> {
    let connection = zbus::Connection::session().await.map_err(|err| format!("the session bus: {err}"))?;
    let accounts = AccountsProxy::new(&connection).await.map_err(|err| err.to_string())?;
    let said = |err: zbus::Error| err.to_string();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["list"] => {
            let list: Vec<model::Account> = serde_json::from_str(&accounts.list().await.map_err(said)?).map_err(|err| err.to_string())?;
            if list.is_empty() {
                println!("No accounts yet. Try: autark-accounts add-google");
            }
            for account in list {
                println!("{}\t{}\t{}\t{}", account.id, account.provider, account.email, if account.has_mail() { "mail" } else { "" });
            }
        }
        ["add-google"] => println!("{}", accounts.add_oauth("google").await.map_err(said)?),
        ["add", provider, email, rest @ ..] => {
            let password = match std::env::var("AUTARK_ACCOUNT_PASSWORD") {
                Ok(password) if !password.is_empty() => password,
                _ => std::io::stdin().lock().lines().next().transpose().map_err(|err| err.to_string())?.unwrap_or_default(),
            };
            println!("{}", accounts.add_password(provider, email, password.trim_end(), rest.first().copied().unwrap_or_default()).await.map_err(said)?);
        }
        ["remove", id] => accounts.remove(id).await.map_err(said)?,
        _ => return Err("usage: autark-accounts [daemon | list | add-google | add <provider> <email> [json] | remove <id>]".to_owned()),
    }
    Ok(())
}
