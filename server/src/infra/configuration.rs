use crate::infra::cli::{GeneralConfigOpts, RunOpts, SmtpOpts, TestEmailOpts};
use anyhow::{Context, Result};
use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use lettre::message::Mailbox;
use lldap_auth::opaque::{server::ServerSetup, KeyPair};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, derive_builder::Builder)]
#[builder(pattern = "owned")]
pub struct MailOptions {
    #[builder(default = "false")]
    pub enable_password_reset: bool,
    #[builder(default = "None")]
    pub from: Option<Mailbox>,
    #[builder(default = "None")]
    pub reply_to: Option<Mailbox>,
    #[builder(default = r#""localhost".to_string()"#)]
    pub server: String,
    #[builder(default = "587")]
    pub port: u16,
    #[builder(default = r#""admin".to_string()"#)]
    pub user: String,
    #[builder(default = r#""".to_string()"#)]
    pub password: String,
    #[builder(default = "true")]
    pub tls_required: bool,
}

impl std::default::Default for MailOptions {
    fn default() -> Self {
        MailOptionsBuilder::default().build().unwrap()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, derive_builder::Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "private_build", validate = "Self::validate")
)]
pub struct Configuration {
    #[builder(default = "3890")]
    pub ldap_port: u16,
    #[builder(default = "6360")]
    pub ldaps_port: u16,
    #[builder(default = "17170")]
    pub http_port: u16,
    #[builder(default = r#"String::from("secretjwtsecret")"#)]
    pub jwt_secret: String,
    #[builder(default = r#"String::from("dc=example,dc=com")"#)]
    pub ldap_base_dn: String,
    #[builder(default = r#"String::from("admin")"#)]
    pub ldap_user_dn: String,
    #[builder(default = r#"String::from("password")"#)]
    pub ldap_user_pass: String,
    #[builder(default = r#"String::from("sqlite://users.db?mode=rwc")"#)]
    pub database_url: String,
    #[builder(default = "false")]
    pub verbose: bool,
    #[builder(default = r#"String::from("server_key")"#)]
    pub key_file: String,
    #[builder(default)]
    pub smtp_options: MailOptions,
    #[serde(skip)]
    #[builder(field(private), setter(strip_option))]
    server_setup: Option<ServerSetup>,
}

impl std::default::Default for Configuration {
    fn default() -> Self {
        ConfigurationBuilder::default().build().unwrap()
    }
}

impl ConfigurationBuilder {
    pub fn build(self) -> Result<Configuration> {
        let server_setup = get_server_setup(self.key_file.as_deref().unwrap_or("server_key"))?;
        Ok(self.server_setup(server_setup).private_build()?)
    }

    fn validate(&self) -> Result<(), String> {
        if self.server_setup.is_none() {
            Err("Don't use `private_build`, use `build` instead".to_string())
        } else {
            Ok(())
        }
    }
}

impl Configuration {
    pub fn get_server_setup(&self) -> &ServerSetup {
        self.server_setup.as_ref().unwrap()
    }

    pub fn get_server_keys(&self) -> &KeyPair {
        self.get_server_setup().keypair()
    }
}

fn get_server_setup(file_path: &str) -> Result<ServerSetup> {
    use std::path::Path;
    let path = Path::new(file_path);
    if path.exists() {
        let bytes =
            std::fs::read(file_path).context(format!("Could not read key file `{}`", file_path))?;
        Ok(ServerSetup::deserialize(&bytes)?)
    } else {
        let mut rng = rand::rngs::OsRng;
        let server_setup = ServerSetup::new(&mut rng);
        std::fs::write(path, server_setup.serialize()).context(format!(
            "Could not write the generated server setup to file `{}`",
            file_path,
        ))?;
        Ok(server_setup)
    }
}

pub trait ConfigOverrider {
    fn override_config(&self, config: &mut Configuration);
}

pub trait TopLevelCommandOpts {
    fn general_config(&self) -> &GeneralConfigOpts;
}

impl TopLevelCommandOpts for RunOpts {
    fn general_config(&self) -> &GeneralConfigOpts {
        &self.general_config
    }
}

impl TopLevelCommandOpts for TestEmailOpts {
    fn general_config(&self) -> &GeneralConfigOpts {
        &self.general_config
    }
}

impl ConfigOverrider for RunOpts {
    fn override_config(&self, config: &mut Configuration) {
        self.general_config.override_config(config);
        if let Some(port) = self.ldap_port {
            config.ldap_port = port;
        }

        if let Some(port) = self.ldaps_port {
            config.ldaps_port = port;
        }

        if let Some(port) = self.http_port {
            config.http_port = port;
        }
        self.smtp_opts.override_config(config);
    }
}

impl ConfigOverrider for TestEmailOpts {
    fn override_config(&self, config: &mut Configuration) {
        self.general_config.override_config(config);
        self.smtp_opts.override_config(config);
    }
}

impl ConfigOverrider for GeneralConfigOpts {
    fn override_config(&self, config: &mut Configuration) {
        if self.verbose {
            config.verbose = true;
        }
    }
}

impl ConfigOverrider for SmtpOpts {
    fn override_config(&self, config: &mut Configuration) {
        if let Some(from) = &self.smtp_from {
            config.smtp_options.from = Some(from.clone());
        }
        if let Some(reply_to) = &self.smtp_reply_to {
            config.smtp_options.reply_to = Some(reply_to.clone());
        }
        if let Some(server) = &self.smtp_server {
            config.smtp_options.server = server.clone();
        }
        if let Some(port) = self.smtp_port {
            config.smtp_options.port = port;
        }
        if let Some(user) = &self.smtp_user {
            config.smtp_options.user = user.clone();
        }
        if let Some(password) = &self.smtp_password {
            config.smtp_options.password = password.clone();
        }
        if let Some(tls_required) = self.smtp_tls_required {
            config.smtp_options.tls_required = tls_required;
        }
    }
}

pub fn init<C>(overrides: C) -> Result<Configuration>
where
    C: TopLevelCommandOpts + ConfigOverrider,
{
    let config_file = overrides.general_config().config_file.clone();

    println!(
        "Loading configuration from {}",
        overrides.general_config().config_file
    );

    let mut config: Configuration = Figment::from(Serialized::defaults(
        ConfigurationBuilder::default().build().unwrap(),
    ))
    .merge(Toml::file(config_file))
    .merge(Env::prefixed("LLDAP_").split("__"))
    .extract()?;

    overrides.override_config(&mut config);
    if config.verbose {
        println!("Configuration: {:#?}", &config);
    }
    config.server_setup = Some(get_server_setup(&config.key_file)?);
    if config.jwt_secret == "secretjwtsecret" {
        println!("WARNING: Default JWT secret used! This is highly unsafe and can allow attackers to log in as admin.");
    }
    if config.ldap_user_pass == "password" {
        println!("WARNING: Unsecure default admin password is used.");
    }
    Ok(config)
}
