//! Typed client for the native UNO dispatch boundary. No Python/UNO bindings.
use crate::{Result, files, launcher};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Duration,
};
pub fn structure(name: &str, fields: Value) -> Value {
    json!({"$struct":format!("com.sun.star.{name}"),"fields":fields})
}
pub fn enumeration(name: &str, value: &str) -> Value {
    json!({"$enum":format!("com.sun.star.{name}"),"name":value})
}
pub fn property(name: &str, value: Value) -> Value {
    structure("beans.PropertyValue", json!({"Name":name,"Value":value}))
}
pub fn file_url(path: &Path) -> Result<String> {
    let p = path.canonicalize().map_err(|e| e.to_string())?;
    let s = p.to_str().ok_or("workbook path is not UTF-8")?;
    let mut url = "file://".to_string();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(url)
}
pub struct Uno {
    bridge: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    office: Child,
}
impl Uno {
    pub fn connect(job: &Path) -> Result<Self> {
        let soffice = launcher::executable("OMASHEETS_SOFFICE", "soffice")?;
        let adapter = launcher::executable("OMASHEETS_UNO_BRIDGE", "omasheets-uno-bridge")?;
        let pipe = format!("omasheets-{}", files::random_id()?);
        let profile = job.join("profile");
        files::private_dir(&profile)?;
        let diagnostic =
            fs::File::create(job.join("soffice-startup.log")).map_err(|e| e.to_string())?;
        let office = Command::new(soffice)
            .args([
                "--headless",
                "--nologo",
                "--nodefault",
                "--norestore",
                "--nofirststartwizard",
                "--nolockcheck",
            ])
            .arg(format!("-env:UserInstallation={}", file_url(&profile)?))
            .arg(format!(
                "--accept=pipe,name={pipe};urp;StarOffice.ComponentContext"
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(diagnostic)
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut bridge = match Command::new(adapter)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(p) => p,
            Err(e) => {
                let mut office = office;
                let _ = office.kill();
                let _ = office.wait();
                return Err(e.to_string());
            }
        };
        let input = bridge.stdin.take().ok_or("UNO stdin unavailable")?;
        let output = BufReader::new(bridge.stdout.take().ok_or("UNO stdout unavailable")?);
        let mut u = Self {
            bridge,
            input,
            output,
            office,
        };
        for _ in 0..100 {
            if u.office.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("LibreOffice exited before opening its private pipe".into());
            }
            if u.exchange(json!({"action":"connect","url":format!("uno:pipe,name={pipe};urp;StarOffice.ComponentContext")})).is_ok(){return Ok(u);}
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("timed out connecting to LibreOffice".into())
    }
    fn exchange(&mut self, request: Value) -> Result<Value> {
        let mut bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("UNO request exceeds limit".into());
        }
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .and_then(|_| self.input.flush())
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![];
        self.output
            .by_ref()
            .take(4 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 || bytes.last() != Some(&b'\n') {
            return Err("UNO returned no bounded result".into());
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if v["ok"] != true {
            return Err(v["error"]
                .as_str()
                .unwrap_or("UNO adapter failed")
                .to_string());
        }
        Ok(v["result"].clone())
    }
    pub fn service(&mut self, name: &str) -> Result<Value> {
        self.exchange(json!({"action":"service","name":format!("com.sun.star.{name}")}))
    }
    pub fn call(&mut self, object: &Value, name: &str, args: Vec<Value>) -> Result<Value> {
        self.exchange(json!({"action":"call","object":object,"name":name,"arguments":args}))
    }
    pub fn get(&mut self, object: &Value, name: &str) -> Result<Value> {
        self.exchange(json!({"action":"get","object":object,"name":name}))
    }
    pub fn set(&mut self, object: &Value, name: &str, value: Value) -> Result<()> {
        self.exchange(json!({"action":"set","object":object,"name":name,"value":value}))
            .map(|_| ())
    }
    pub fn release(&mut self, object: &Value) -> Result<()> {
        self.exchange(json!({"action":"release","object":object}))
            .map(|_| ())
    }
    pub fn load(&mut self, path: &Path, read_only: bool) -> Result<Value> {
        let desktop = self.service("frame.Desktop")?;
        let doc = self.call(
            &desktop,
            "loadComponentFromURL",
            vec![
                json!(file_url(path)?),
                json!("_blank"),
                json!(0),
                json!([
                    property("Hidden", json!(true)),
                    property("ReadOnly", json!(read_only)),
                    property("MacroExecutionMode", json!(0)),
                    property("UpdateDocMode", json!(0)),
                    property("Silent", json!(true))
                ]),
            ],
        )?;
        self.release(&desktop)?;
        if doc.is_null()
            || self.call(
                &doc,
                "supportsService",
                vec![json!("com.sun.star.sheet.SpreadsheetDocument")],
            )? != true
        {
            return Err("input is not a Calc spreadsheet".into());
        }
        Ok(doc)
    }
}
impl Drop for Uno {
    fn drop(&mut self) {
        for p in [&mut self.bridge, &mut self.office] {
            let _ = p.kill();
            let _ = p.wait();
        }
    }
}
