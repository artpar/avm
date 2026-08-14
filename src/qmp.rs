use std::{path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf},
    net::UnixStream,
    time::{Instant, sleep},
};

pub struct QmpClient {
    reader: BufReader<ReadHalf<UnixStream>>,
    writer: WriteHalf<UnixStream>,
    next_id: u64,
}

impl QmpClient {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let stream = UnixStream::connect(path)
            .await
            .with_context(|| format!("connect QMP socket {}", path.display()))?;
        let (reader, writer) = tokio::io::split(stream);
        let mut client = Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        };
        let greeting = client.read_message().await?;
        if greeting.get("QMP").is_none() {
            bail!("invalid QMP greeting: {greeting}");
        }
        client.execute("qmp_capabilities", None).await?;
        Ok(client)
    }

    pub async fn execute(&mut self, command: &str, arguments: Option<Value>) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"execute": command, "id": id});
        if let Some(arguments) = arguments {
            request["arguments"] = arguments;
        }
        let mut encoded = serde_json::to_vec(&request)?;
        encoded.push(b'\n');
        self.writer.write_all(&encoded).await?;
        self.writer.flush().await?;

        loop {
            let response = self.read_message().await?;
            if response.get("id") != Some(&json!(id)) {
                // QMP events are asynchronous and may arrive between request and response.
                continue;
            }
            if let Some(error) = response.get("error") {
                bail!("QMP {command} failed: {error}");
            }
            return Ok(response.get("return").cloned().unwrap_or(Value::Null));
        }
    }

    pub async fn snapshot_save(
        &mut self,
        job_id: &str,
        tag: &str,
        vmstate: &str,
        devices: &[&str],
    ) -> Result<()> {
        self.execute_job(
            "snapshot-save",
            json!({
                "job-id": job_id,
                "tag": tag,
                "vmstate": vmstate,
                "devices": devices,
            }),
            job_id,
        )
        .await
    }

    pub async fn snapshot_load(
        &mut self,
        job_id: &str,
        tag: &str,
        vmstate: &str,
        devices: &[&str],
    ) -> Result<()> {
        self.execute_job(
            "snapshot-load",
            json!({
                "job-id": job_id,
                "tag": tag,
                "vmstate": vmstate,
                "devices": devices,
            }),
            job_id,
        )
        .await
    }

    async fn execute_job(&mut self, command: &str, arguments: Value, job_id: &str) -> Result<()> {
        self.execute(command, Some(arguments)).await?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let jobs = self.execute("query-jobs", None).await?;
            let jobs = jobs
                .as_array()
                .context("QMP query-jobs returned a non-array")?;
            if let Some(job) = jobs.iter().find(|job| job["id"] == job_id) {
                if job["status"] == "concluded" {
                    let error = job.get("error").and_then(Value::as_str).map(str::to_owned);
                    self.execute("job-dismiss", Some(json!({"id": job_id})))
                        .await?;
                    if let Some(error) = error {
                        bail!("QMP {command} job {job_id} failed: {error}");
                    }
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for QMP {command} job {job_id}");
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    async fn read_message(&mut self) -> Result<Value> {
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).await?;
            if read == 0 {
                bail!("QMP connection closed");
            }
            if !line.trim().is_empty() {
                return serde_json::from_str(&line).context("decode QMP JSON");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    #[tokio::test]
    async fn negotiates_capabilities_and_ignores_interleaved_events() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("qmp.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut read = BufReader::new(read);
            write
                .write_all(b"{\"QMP\":{\"version\":{}}}\n")
                .await
                .unwrap();
            let mut request = String::new();
            read.read_line(&mut request).await.unwrap();
            let id = serde_json::from_str::<Value>(&request).unwrap()["id"].clone();
            write
                .write_all(format!("{{\"return\":{{}},\"id\":{id}}}\n").as_bytes())
                .await
                .unwrap();
            request.clear();
            read.read_line(&mut request).await.unwrap();
            let id = serde_json::from_str::<Value>(&request).unwrap()["id"].clone();
            write.write_all(b"{\"event\":\"RESET\"}\n").await.unwrap();
            write
                .write_all(
                    format!("{{\"return\":{{\"status\":\"running\"}},\"id\":{id}}}\n").as_bytes(),
                )
                .await
                .unwrap();
        });

        let mut client = QmpClient::connect(&socket).await.unwrap();
        let status = client.execute("query-status", None).await.unwrap();
        assert_eq!(status["status"], "running");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_job_waits_for_conclusion_and_is_dismissed() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("qmp.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut read = BufReader::new(read);
            write
                .write_all(b"{\"QMP\":{\"version\":{}}}\n")
                .await
                .unwrap();
            reply_to_next(&mut read, &mut write, json!({})).await;

            let request = reply_to_next(&mut read, &mut write, json!({})).await;
            assert_eq!(request["execute"], "snapshot-save");
            assert_eq!(request["arguments"]["tag"], "clean");
            assert_eq!(request["arguments"]["vmstate"], "os");
            assert_eq!(request["arguments"]["devices"], json!(["os"]));

            reply_to_next(
                &mut read,
                &mut write,
                json!([{"id":"save-1","type":"snapshot-save","status":"running"}]),
            )
            .await;
            reply_to_next(
                &mut read,
                &mut write,
                json!([{"id":"save-1","type":"snapshot-save","status":"concluded"}]),
            )
            .await;
            let dismiss = reply_to_next(&mut read, &mut write, json!({})).await;
            assert_eq!(dismiss["execute"], "job-dismiss");
            assert_eq!(dismiss["arguments"]["id"], "save-1");
        });

        let mut client = QmpClient::connect(&socket).await.unwrap();
        client
            .snapshot_save("save-1", "clean", "os", &["os"])
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_job_surfaces_terminal_error_after_dismissal() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("qmp.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut read = BufReader::new(read);
            write
                .write_all(b"{\"QMP\":{\"version\":{}}}\n")
                .await
                .unwrap();
            reply_to_next(&mut read, &mut write, json!({})).await;
            reply_to_next(&mut read, &mut write, json!({})).await;
            reply_to_next(
                &mut read,
                &mut write,
                json!([{"id":"load-1","type":"snapshot-load","status":"concluded","error":"snapshot missing"}]),
            )
            .await;
            reply_to_next(&mut read, &mut write, json!({})).await;
        });

        let mut client = QmpClient::connect(&socket).await.unwrap();
        let error = client
            .snapshot_load("load-1", "clean", "os", &["os"])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("snapshot missing"));
        server.await.unwrap();
    }

    async fn reply_to_next<R, W>(read: &mut BufReader<R>, write: &mut W, value: Value) -> Value
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut request = String::new();
        read.read_line(&mut request).await.unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        let id = &request["id"];
        write
            .write_all(
                serde_json::to_string(&json!({"return": value, "id": id}))
                    .unwrap()
                    .as_bytes(),
            )
            .await
            .unwrap();
        write.write_all(b"\n").await.unwrap();
        request
    }
}
