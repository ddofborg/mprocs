use std::{env::consts::OS, fs::File, io::BufReader, path::Path, rc::Rc};

use anyhow::bail;
use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};

use indexmap::IndexMap;
use serde_yaml::Value;

pub struct Config {
  pub procs: Vec<ProcConfig>,
  pub server: Option<ServerConfig>,
}

impl Config {
  pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Config> {
    // Open the file in read-only mode with buffer.
    let file = match File::open(&path) {
      Ok(file) => file,
      Err(err) => match err.kind() {
        std::io::ErrorKind::NotFound => {
          return Err(anyhow::anyhow!(
            "Config file '{}' not found.",
            path.as_ref().display()
          ))
        }
        _kind => return Err(err.into()),
      },
    };
    let reader = BufReader::new(file);

    let config: Value = serde_yaml::from_reader(reader)?;
    let config = Val::new(&config)?;
    let config = config.as_object()?;

    let procs = if let Some(procs) = config.get(&Value::from("procs")) {
      let procs = procs
        .as_object()?
        .into_iter()
        .map(|(name, proc)| {
          Ok(ProcConfig::from_val(value_to_string(&name)?, proc)?)
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|x| x)
        .collect::<Vec<_>>();
      procs
    } else {
      Vec::new()
    };

    let server = if let Some(addr) = config.get(&Value::from("server")) {
      Some(ServerConfig::from_str(addr.as_str()?)?)
    } else {
      None
    };

    let config = Config { procs, server };

    Ok(config)
  }
}

impl Default for Config {
  fn default() -> Self {
    Self {
      procs: Vec::new(),
      server: None,
    }
  }
}

pub struct ProcConfig {
  pub name: String,
  pub cmd: CmdConfig,
  pub cwd: Option<String>,
  pub env: Option<IndexMap<String, Option<String>>>,
}

impl ProcConfig {
  fn from_val(name: String, val: Val) -> anyhow::Result<Option<ProcConfig>> {
    match val.0 {
      Value::Null => Ok(None),
      Value::Bool(_) => todo!(),
      Value::Number(_) => todo!(),
      Value::String(shell) => Ok(Some(ProcConfig {
        name,
        cmd: CmdConfig::Shell {
          shell: shell.to_owned(),
        },
        cwd: None,
        env: None,
      })),
      Value::Sequence(_) => {
        let cmd = val.as_array()?;
        let cmd = cmd
          .into_iter()
          .map(|item| item.as_str().map(|s| s.to_owned()))
          .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Some(ProcConfig {
          name,
          cmd: CmdConfig::Cmd { cmd },
          cwd: None,
          env: None,
        }))
      }
      Value::Mapping(_) => {
        let map = val.as_object()?;

        let cmd = {
          let shell = map.get(&Value::from("shell"));
          let cmd = map.get(&Value::from("cmd"));

          match (shell, cmd) {
            (None, Some(cmd)) => CmdConfig::Cmd {
              cmd: cmd
                .as_array()?
                .into_iter()
                .map(|v| v.as_str().map(|s| s.to_owned()))
                .collect::<anyhow::Result<Vec<_>>>()?,
            },
            (Some(shell), None) => CmdConfig::Shell {
              shell: shell.as_str()?.to_owned(),
            },
            (None, None) => todo!(),
            (Some(_), Some(_)) => todo!(),
          }
        };

        let env = match map.get(&Value::from("env")) {
          Some(env) => {
            let env = env.as_object()?;
            let env = env
              .into_iter()
              .map(|(k, v)| {
                let v = match v.0 {
                  Value::Null => Ok(None),
                  Value::String(v) => Ok(Some(v.to_owned())),
                  _ => Err(v.error_at("Expected string or null")),
                };
                Ok((value_to_string(&k)?, v?))
              })
              .collect::<anyhow::Result<IndexMap<_, _>>>()?;
            Some(env)
          }
          None => None,
        };

        Ok(Some(ProcConfig {
          name,
          cmd,
          cwd: None,
          env,
        }))
      }
    }
  }
}

pub enum ServerConfig {
  Tcp(String),
}

impl ServerConfig {
  pub fn from_str(server_addr: &str) -> anyhow::Result<Self> {
    Ok(Self::Tcp(server_addr.to_string()))
  }
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub enum CmdConfig {
  Cmd { cmd: Vec<String> },
  Shell { shell: String },
}

impl From<&ProcConfig> for CommandBuilder {
  fn from(cfg: &ProcConfig) -> Self {
    let mut cmd = match &cfg.cmd {
      CmdConfig::Cmd { cmd } => {
        let (head, tail) = cmd.split_at(1);
        let mut cmd = CommandBuilder::new(&head[0]);
        cmd.args(tail);
        cmd
      }
      CmdConfig::Shell { shell } => {
        if cfg!(target_os = "windows") {
          let mut cmd = CommandBuilder::new("cmd");
          cmd.args(["/C", &shell]);
          cmd
        } else {
          let mut cmd = CommandBuilder::new("sh");
          cmd.arg("-c");
          cmd.arg(&shell);
          cmd
        }
      }
    };

    if let Some(env) = &cfg.env {
      for (k, v) in env {
        if let Some(v) = v {
          cmd.env(k, v);
        } else {
          cmd.env_remove(k);
        }
      }
    }

    let cwd = match &cfg.cwd {
      Some(cwd) => Some(cwd.clone()),
      None => std::env::current_dir()
        .ok()
        .map(|cd| cd.as_path().to_string_lossy().to_string()),
    };
    if let Some(cwd) = cwd {
      cmd.cwd(cwd);
    }

    cmd
  }
}

#[derive(Clone)]
struct Trace(Option<Rc<Box<(String, Trace)>>>);

impl Trace {
  pub fn empty() -> Self {
    Trace(None)
  }

  pub fn add<T: ToString>(&self, seg: T) -> Self {
    Trace(Some(Rc::new(Box::new((seg.to_string(), self.clone())))))
  }

  pub fn to_string(&self) -> String {
    let mut str = String::new();
    fn add(buf: &mut String, trace: &Trace) {
      match &trace.0 {
        Some(part) => {
          add(buf, &part.1);
          buf.push('.');
          buf.push_str(&part.0);
        }
        None => buf.push_str("<config>"),
      }
    }
    add(&mut str, self);

    str
  }
}

struct Val<'a>(&'a Value, Trace);

impl<'a> Val<'a> {
  pub fn new(value: &'a Value) -> anyhow::Result<Self> {
    Self::create(value, Trace::empty())
  }

  pub fn create(value: &'a Value, trace: Trace) -> anyhow::Result<Self> {
    match value {
      Value::Mapping(map) => {
        if map
          .into_iter()
          .next()
          .map_or(false, |(k, _)| k.eq("$select"))
        {
          let (v, t) = Self::select(map, trace.clone())?;
          return Self::create(v, t);
        }
      }
      _ => (),
    }
    Ok(Val(value, trace))
  }

  fn select(
    map: &'a serde_yaml::Mapping,
    trace: Trace,
  ) -> anyhow::Result<(&'a Value, Trace)> {
    if map.get(&Value::from("$select")).unwrap() == "os" {
      if let Some(v) = map.get(&Value::from(OS)) {
        return Ok((v, trace.add(OS)));
      }

      if let Some(v) = map.get(&Value::from("$else")) {
        return Ok((v, trace.add("$else")));
      }

      anyhow::bail!(
        "No matching condition found at {}. Use \"$else\" for default value.",
        trace.to_string(),
      )
    } else {
      anyhow::bail!("Expected \"os\" at {}", trace.add("$select").to_string())
    }
  }

  pub fn error_at<T: AsRef<str>>(&self, msg: T) -> anyhow::Error {
    anyhow::format_err!("{} at {}", msg.as_ref(), self.1.to_string())
  }

  pub fn as_str(&self) -> anyhow::Result<&str> {
    self.0.as_str().ok_or_else(|| {
      anyhow::format_err!("Expected string at {}", self.1.to_string())
    })
  }

  pub fn as_array(&self) -> anyhow::Result<Vec<Val>> {
    self
      .0
      .as_sequence()
      .ok_or_else(|| {
        anyhow::format_err!("Expected array at {}", self.1.to_string())
      })?
      .iter()
      .enumerate()
      .map(|(i, item)| Val::create(item, self.1.add(i)))
      .collect::<anyhow::Result<Vec<_>>>()
  }

  pub fn as_object(&self) -> anyhow::Result<IndexMap<Value, Val>> {
    self
      .0
      .as_mapping()
      .ok_or_else(|| {
        anyhow::format_err!("Expected object at {}", self.1.to_string())
      })?
      .iter()
      .map(|(k, item)| {
        #[inline]
        fn mk_val<'a>(
          k: &'a Value,
          item: &'a Value,
          trace: &'a Trace,
        ) -> anyhow::Result<Val<'a>> {
          Ok(Val::create(item, trace.add(value_to_string(k)?))?)
        }
        Ok((k.to_owned(), mk_val(k, item, &self.1)?))
      })
      .collect::<anyhow::Result<IndexMap<_, _>>>()
  }
}

fn value_to_string(value: &Value) -> anyhow::Result<String> {
  match value {
    Value::Null => Ok("null".to_string()),
    Value::Bool(v) => Ok(v.to_string()),
    Value::Number(v) => Ok(v.to_string()),
    Value::String(v) => Ok(v.to_string()),
    Value::Sequence(_v) => {
      bail!("`value_to_string` is not implemented for arrays.")
    }
    Value::Mapping(_v) => {
      bail!("`value_to_string` is not implemented for objects.")
    }
  }
}
