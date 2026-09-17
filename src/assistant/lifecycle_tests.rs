use super::*;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
#[ignore = "requires explicitly selected live backend via R105_TEST_URL and R105_TEST_MODEL"]
async fn live_backend_stream_and_approved_write() {
    let Ok(url) = std::env::var("R105_TEST_URL") else {
        eprintln!("skipping live backend test: set R105_TEST_URL and R105_TEST_MODEL");
        return;
    };
    let Ok(model) = std::env::var("R105_TEST_MODEL") else {
        eprintln!("skipping live backend test: set R105_TEST_URL and R105_TEST_MODEL");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::Config::default();
    let mut state = ChatState::from_config(&config, dir.path().into());
    state.model = model.clone();
    state.mode = "build".into();
    state.max_tokens = Some(4096);
    state.skills_dir = dir.path().join("skills");
    state.config_dir = dir.path().join("config");
    let mut parts = AssistantParts::from_config(&config);
    parts.plugins_dir = dir.path().join("plugins");
    parts.policy = Policy::locked_down();
    parts.policy.write = approve::Action::Ask;
    let backend = Backend::new(
        crate::backend::Connection {
            provider_id: None,
            backend: "direct".into(),
            base_url: url,
            api_key: None,
            model,
        },
        90,
    )
    .unwrap();
    let mut assistant = spawn_assistant(backend, state, parts);
    let mut approvals = 0;
    let mut tokens = 0;
    tokio::time::timeout(std::time::Duration::from_secs(180),async {
        for prompt in ["Reply with exactly READY. Do not use tools.","Use write_file to create acceptance.txt containing exactly native-window-ok (no newline). Do not use shell or other tools. After the tool succeeds reply SAVED."] {
            assistant.ask(prompt.into());
            loop {
                match assistant.events.recv().await.unwrap() {
                    AssistantEvent::Token(text) => { tokens += 1; print!("{text}"); }
                    AssistantEvent::ApprovalNeeded { id, name, .. } => {
                        assert_eq!(name,"write_file");
                        approvals += 1;
                        assistant.verdict(id,ApprovalVerdict::Once);
                    }
                    AssistantEvent::Done { .. } => { println!(); break; }
                    AssistantEvent::Error(error) => panic!("{error}"),
                    _ => {},
                }
            }
        }
    }).await.unwrap();
    assert!(tokens > 0, "must receive streamed tokens");
    assert_eq!(approvals, 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("acceptance.txt")).unwrap(),
        "native-window-ok"
    );
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = Vec::new();
    let (header_end, length) = loop {
        let mut chunk = [0; 4096];
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..at]);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            break (at + 4, length);
        }
    };
    while bytes.len() < header_end + length {
        let mut chunk = [0; 4096];
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
}

#[tokio::test]
async fn approvals_execute_real_writes_and_keep_their_lifetimes() {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        for verdict in [ApprovalVerdict::Once,ApprovalVerdict::Always,ApprovalVerdict::Deny] {
            let dir = tempfile::tempdir().unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let mut requests = Vec::new();
                for index in 0..4 {
                    let (mut socket,_) = listener.accept().await.unwrap();
                    requests.push(read_request(&mut socket).await);
                    let delta = if index % 2 == 0 { json!({"reasoning_content":"decide", "tool_calls":[{
                        "index":0,"id":format!("call-{index}"),"type":"function", "function":{"name":"write_file","arguments":json!({"path":"note.txt","content":"approved"}).to_string()}
                    }]}) } else { json!({"content":"finished"}) };
                    let body = format!("data: {}\n\ndata: [DONE]\n\n",json!({"choices":[{"delta":delta}]}));
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                    socket.write_all(response.as_bytes()).await.unwrap();
                }
                requests
            });
            let config = crate::config::Config::default();
            let mut state = ChatState::from_config(&config,dir.path().to_path_buf());
            state.mode = "build".into();
            let mut parts = AssistantParts::from_config(&config);
            parts.plugins_dir = dir.path().join("plugins");
            parts.policy.write = approve::Action::Ask;
            let paths = crate::config::ConfigPaths {
                home:dir.path().into(),config_dir:dir.path().into(),config_file:dir.path().join("config.json"),sessions_dir:dir.path().join("sessions"),plugins_dir:dir.path().join("plugins"),
            };
            parts.persistence = Some((paths.clone(),"window-test".into()));
            let backend = Backend::new(crate::backend::Connection {
                provider_id:None,backend:"direct".into(),base_url:format!("http://{address}"),api_key:None,model:"test".into(),
            },5).unwrap();
            let mut assistant = spawn_assistant(backend,state.clone(),parts);
            let mut cards = 0;
            for turn in 0..2 {
                assistant.ask(format!("write {turn}"));
                loop {
                    match assistant.events.recv().await.unwrap() {
                        AssistantEvent::ApprovalNeeded { id, .. } => {
                            cards += 1;
                            assistant.verdict(id.clone(),verdict);
                            // A repeated key event must not resolve the next card.
                            assistant.verdict(id,ApprovalVerdict::Always);
                        }
                        AssistantEvent::Done { response, .. } => { assert_eq!(response,"finished"); break; }
                        AssistantEvent::Error(error) => panic!("{error}"),
                        _ => {},
                    }
                }
                if verdict == ApprovalVerdict::Deny { assert!(!dir.path().join("note.txt").exists()); }
                else { assert_eq!(std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),"approved"); }
            }
            assert_eq!(cards,if verdict == ApprovalVerdict::Always { 1 } else { 2 });
            let requests = server.await.unwrap();
            for request in [&requests[1],&requests[3]] {
                let messages = request["messages"].as_array().unwrap();
                assert!(messages.iter().any(|m| m["role"] == "tool"));
                assert!(messages.iter().any(|m| m["reasoning_content"] == "decide"));
            }
            crate::session::load(&paths,"window-test",&mut state).unwrap();
            let history = AiHistory::from_messages(&state.history);
            assert_eq!(history.len(),2);
            assert_eq!(history.list()[1].response,"finished");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn cancellation_before_task_starts_cancels_the_queued_request() {
    let config = crate::config::Config::default();
    let backend = Backend::new(
        crate::backend::Connection {
            provider_id: None,
            backend: "direct".into(),
            base_url: "http://127.0.0.1:9".into(),
            api_key: None,
            model: "test".into(),
        },
        5,
    )
    .unwrap();
    let mut assistant = spawn_assistant(
        backend,
        ChatState::from_config(&config, std::env::temp_dir()),
        AssistantParts::from_config(&config),
    );
    assistant.ask("cancel immediately".into());
    assistant.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let AssistantEvent::Error(error) = assistant.events.recv().await.unwrap() {
                assert!(error.contains("cancelled"), "{error}");
                break;
            }
        }
    })
    .await
    .unwrap();
}
