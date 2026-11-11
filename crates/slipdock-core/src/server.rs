//! Asks the server what exists, through the mysql client.
//!
//! This is a convenience for picking names from a list. It is optional:
//! without a working mysql client, or without a connection, the front ends
//! let the user type the names instead.

use std::time::Duration;

use crate::exec::{CaptureError, capture};
use crate::job::{self, Context};
use crate::profile::Profile;
use crate::tools::{self, Tool};
use crate::{Error, Result, command};

const QUERY_LIMIT: Duration = Duration::from_secs(10);

/// Databases that belong to the server itself and are not offered for dumps.
const SYSTEM_DATABASES: [&str; 4] = ["information_schema", "performance_schema", "mysql", "sys"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    /// mydumper dumps views too, so they can be excluded like tables.
    pub is_view: bool,
}

pub fn databases(ctx: &Context, profile: &Profile) -> Result<Vec<String>> {
    let output = query(ctx, profile, "SHOW DATABASES")?;
    Ok(parse_databases(&output))
}

pub fn tables(ctx: &Context, profile: &Profile, database: &str) -> Result<Vec<Table>> {
    // A backtick inside a name is written twice.
    let name = database.replace('`', "``");
    let output = query(ctx, profile, &format!("SHOW FULL TABLES FROM `{name}`"))?;
    Ok(parse_tables(&output))
}

fn query(ctx: &Context, profile: &Profile, sql: &str) -> Result<String> {
    let mysql = tools::resolve(Tool::Mysql, &ctx.settings, &ctx.dirs)?;
    let defaults = job::defaults_for(ctx, profile, None, true)?;
    let invocation = command::mysql_query(&mysql, defaults.path(), sql);
    match capture(&mut invocation.to_command(), QUERY_LIMIT) {
        Ok(output) if output.success() => Ok(output.stdout),
        Ok(output) => {
            let reason = output.stderr.lines().next().unwrap_or("no message").trim();
            Err(Error::msg(format!(
                "could not ask {}: {reason}",
                profile.host()
            )))
        }
        Err(CaptureError::TimedOut) => Err(Error::msg(format!(
            "no answer from {} within {} s",
            profile.host(),
            QUERY_LIMIT.as_secs()
        ))),
        Err(CaptureError::NotStarted(error)) => Err(Error::msg(format!(
            "cannot run {}: {error}",
            mysql.display()
        ))),
    }
}

fn parse_databases(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !SYSTEM_DATABASES.contains(name))
        .map(str::to_string)
        .collect()
}

/// Each line is `name<TAB>BASE TABLE` or `name<TAB>VIEW`.
fn parse_tables(output: &str) -> Vec<Table> {
    output
        .lines()
        .filter_map(|line| {
            let mut columns = line.split('\t');
            let name = columns.next()?.trim();
            (!name.is_empty()).then(|| Table {
                name: name.to_string(),
                is_view: columns.next().map(str::trim) == Some("VIEW"),
            })
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::job::testing::*;
    use crate::secrets::SecretStore;
    use std::fs;

    #[test]
    fn output_is_parsed() {
        assert_eq!(
            parse_databases(
                "information_schema\nexample_db\nmysql\nother_db\nperformance_schema\nsys\n"
            ),
            ["example_db", "other_db"]
        );
        assert_eq!(
            parse_tables("users\tBASE TABLE\nreport\tVIEW\nplain\n\n"),
            [
                Table {
                    name: "users".into(),
                    is_view: false
                },
                Table {
                    name: "report".into(),
                    is_view: true
                },
                Table {
                    name: "plain".into(),
                    is_view: false
                },
            ]
        );
    }

    /// A mysql that passes the probe, records its arguments and the defaults
    /// file it was given, and answers queries from fixed text.
    fn fake_mysql(bin: &std::path::Path, record: &std::path::Path) {
        let body = format!(
            r#"case "$1" in
  --defaults-file=*) file="${{1#--defaults-file=}}" ;;
  *) echo "mysql  Ver 9.7.1 for example on arm64"; exit 0 ;;
esac
if [ "$2" = --print-defaults ]; then sed -n 's/^user=/--user=/p' "$file"; exit 0; fi
printf '%s\n' "$@" > "{record}.args"
cat "$file" > "{record}.cnf"
case "$5" in
  "SHOW DATABASES") printf 'information_schema\nexample_db\n' ;;
  SHOW\ FULL*) printf 'users\tBASE TABLE\nreport\tVIEW\n' ;;
  *) echo "ERROR 1045 (28000): Access denied" >&2; exit 1 ;;
esac"#,
            record = record.display()
        );
        fake_tool(bin, "mysql", &body);
    }

    #[test]
    fn databases_and_tables_come_from_the_client() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, secrets) = context(temp.path());
        let record = temp.path().join("record");
        fake_mysql(&ctx.dirs.path[0], &record);
        write_profile(
            &ctx,
            "example",
            "[client]\nhost = db.example.test\nuser = u\n\n[slipdock]\npassword-store = keychain\n",
        );
        secrets.set("example", "from-keychain").unwrap();
        let profile = ctx.profile("example").unwrap();

        assert_eq!(databases(&ctx, &profile).unwrap(), ["example_db"]);
        let tables = tables(&ctx, &profile, "odd`name").unwrap();
        assert_eq!(tables.len(), 2);
        assert!(tables[1].is_view);

        let args = fs::read_to_string(temp.path().join("record.args")).unwrap();
        assert!(
            args.ends_with("-N\n-B\n-e\nSHOW FULL TABLES FROM `odd``name`\n"),
            "{args}"
        );
        // The keychain password reached the client through the private copy.
        let cnf = fs::read_to_string(temp.path().join("record.cnf")).unwrap();
        assert!(cnf.contains("password = from-keychain"));
    }

    #[test]
    fn a_refused_query_gives_the_servers_reason() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, _) = context(temp.path());
        fake_mysql(&ctx.dirs.path[0], &temp.path().join("record"));
        write_profile(&ctx, "example", "[client]\nhost = db.example.test\n");
        let profile = ctx.profile("example").unwrap();
        // The fake only knows the two SHOW statements.
        let error = query(&ctx, &profile, "SELECT 1").unwrap_err().to_string();
        assert_eq!(
            error,
            "could not ask db.example.test: ERROR 1045 (28000): Access denied"
        );
    }

    #[test]
    fn without_a_client_the_error_points_to_install() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, _) = context(temp.path());
        write_profile(&ctx, "example", "[client]\nhost = h\n");
        let profile = ctx.profile("example").unwrap();
        let error = databases(&ctx, &profile).unwrap_err().to_string();
        assert!(error.contains("slipdock tools install mysql"), "{error}");
    }
}
