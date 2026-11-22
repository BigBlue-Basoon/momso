use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    thread,
    time::Duration,
};

fn emit(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn main() {
    let mut crm_url = String::new();
    let mut public_reads = 0;
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("logout") {
        assert!(std::env::var_os("MOMSO_SERVER_TOKEN").is_none());
        let home = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        match std::fs::remove_file(home.join("auth.json")) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => std::process::exit(1),
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("login") {
        let home = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        if args.get(2).map(String::as_str) == Some("status") {
            std::process::exit(if home.join("auth.json").exists() {
                0
            } else {
                1
            });
        }
        let pid = std::process::id();
        std::fs::write(home.join(format!("fixture-login-start-{pid}")), "").unwrap();
        println!("1. Open this link in your browser and sign in to your account\nhttps://auth.openai.com/codex/device\n2. Enter this one-time code (expires in 15 minutes)\nTEST-1234");
        io::stdout().flush().unwrap();
        thread::sleep(Duration::from_secs(2));
        std::fs::write(home.join(format!("fixture-login-finish-{pid}")), "").unwrap();
        std::fs::write(home.join("auth.json"), "fixture").unwrap();
        return;
    }
    assert!(std::env::var_os("MOMSO_SERVER_TOKEN").is_none());
    for line in io::stdin().lock().lines() {
        let Ok(value) = serde_json::from_str::<Value>(&line.unwrap()) else {
            continue;
        };
        match value.get("id").and_then(Value::as_u64) {
            Some(1) => emit(json!({"id":1,"result":{}})),
            Some(2) if value["method"] == "model/list" => emit(
                json!({"id":2,"result":{"data":[{"model":"gpt-6.1-sol","displayName":"Fixture","isDefault":true,"supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"ultra"}]}],"nextCursor":null}}),
            ),
            Some(2) if value["method"] == "account/read" => emit(
                json!({"id":2,"result":{"account":{"type":"chatgpt","email":"teacher@example.test"}}}),
            ),
            Some(2) => {
                if value["method"] == "thread/start" && cfg!(target_os = "linux") {
                    assert!(value
                        .pointer("/params/dynamicTools")
                        .and_then(Value::as_array)
                        .is_some_and(|a| a.iter().any(|t| t["name"] == "browser_navigate")));
                }
                emit(json!({"id":2,"result":{"thread":{"id":"test-thread"}}}));
            }
            Some(3) if value["method"] == "account/rateLimits/read" => emit(
                json!({"id":3,"result":{"rateLimits":{"primary":{"usedPercent":25.0,"windowDurationMins":300,"resetsAt":1800000000}}}}),
            ),
            Some(3) => {
                if value.to_string().contains("fast unavailable fixture") {
                    emit(json!({"id":3,"error":{"code":-32600,"message":"fast unavailable"}}));
                    continue;
                }
                if value.to_string().contains("model fixture") {
                    assert_eq!(value["params"]["model"], "gpt-6.1-sol");
                    assert_eq!(value["params"]["serviceTierForTurn"], "fast");
                    assert_eq!(value["params"]["effort"], "ultra");
                }
                emit(json!({"id":3,"result":{}}));
                let input = value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if input.contains("generated tool fixture")
                    || input.contains("generated scan fixture")
                    || input.contains("regenerate fixture")
                {
                    let directory = input
                        .lines()
                        .find(|line| line.contains("생성한 사진과 동영상은"))
                        .unwrap()
                        .rsplit_once(": ")
                        .unwrap()
                        .1;
                    let path = std::path::Path::new(directory).join("fixture.png");
                    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01".to_vec();
                    bytes.extend_from_slice(if input.contains("regenerate fixture") {
                        b"second"
                    } else if input.contains("generated scan fixture") {
                        b"scan!!"
                    } else {
                        b"first!"
                    });
                    std::fs::write(&path, bytes).unwrap();
                    if input.contains("generated scan fixture") {
                        emit(
                            json!({"method":"item/agentMessage/delta","params":{"delta":"scan ready"}}),
                        );
                        emit(
                            json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                        );
                    } else {
                        emit(
                            json!({"id":306,"method":"item/tool/call","params":{"tool":"media_register_generated","arguments":{"path":path,"prompt":"draw fixture"}}}),
                        );
                    }
                    continue;
                }
                if value.to_string().contains("context fixture") {
                    emit(
                        json!({"method":"thread/tokenUsage/updated","params":{"threadId":"other-thread","tokenUsage":{"last":{"totalTokens":9999},"modelContextWindow":10000}}}),
                    );
                    emit(
                        json!({"method":"thread/tokenUsage/updated","params":{"threadId":"test-thread","tokenUsage":{"last":{"totalTokens":123},"modelContextWindow":1000}}}),
                    );
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":"context fixture done"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                    continue;
                }
                if value.to_string().contains("profile fixture") {
                    assert!(value.to_string().contains("선생님"));
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":"profile received"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                    continue;
                }
                if value.to_string().contains("handoff fixture") {
                    assert!(value.to_string().contains("첫 말"));
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":"handoff delivered"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                    continue;
                }
                if value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.contains("s75-skill-fixture-marker"))
                {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":"skill fixture accepted"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                    continue;
                }
                if let Some(input) = value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .filter(|text| text.starts_with("attachment fixture"))
                {
                    let context = input
                        .split_once("자료): ")
                        .and_then(|(_, v)| serde_json::from_str::<Value>(v).ok())
                        .unwrap();
                    let path = context[0]["path"].as_str().unwrap();
                    let content = std::fs::read_to_string(path).unwrap();
                    emit(json!({"method":"item/agentMessage/delta","params":{"delta":content}}));
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                    continue;
                }
                if value.to_string().contains("document fixture") {
                    emit(
                        json!({"id":305,"method":"item/tool/call","params":{"tool":"momso_document_create","arguments":{"title":"가짜 문서","summary":"가짜 요약","body":"가짜 본문","folder":"수업"}}}),
                    );
                    continue;
                }
                if value.to_string().contains("crm direct fixture") {
                    emit(
                        json!({"id":303,"method":"item/tool/call","params":{"tool":"momso_crm_upsert","arguments":{"records":[{"kind":"class","source":{"service":"fixture","externalId":"class-1","observedAt":"2026-11-21T09:00:00+09:00"},"data":{"name":"가짜 수업"}}]}}}),
                    );
                    continue;
                }
                if let Some(url) = value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("crm fixture "))
                {
                    crm_url = url.to_owned();
                    emit(
                        json!({"id":301,"method":"item/tool/call","params":{"tool":"momso_skill_read","arguments":{"name":"bodycodi-operations"}}}),
                    );
                    continue;
                }
                if value.to_string().contains("crm lookup fixture") {
                    emit(
                        json!({"id":304,"method":"item/tool/call","params":{"tool":"momso_crm_search","arguments":{"kind":"class"}}}),
                    );
                    continue;
                }
                if let Some(url) = value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("login fixture "))
                {
                    emit(
                        json!({"id":221,"method":"item/tool/call","params":{"tool":"browser_navigate","arguments":{"url":url}}}),
                    );
                    continue;
                }
                if let Some(url) = value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("otp fixture "))
                {
                    emit(
                        json!({"id":211,"method":"item/tool/call","params":{"tool":"browser_navigate","arguments":{"url":url}}}),
                    );
                    continue;
                }
                if let Some(url) = value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("read fixture "))
                {
                    emit(
                        json!({"id":214,"method":"item/tool/call","params":{"tool":"browser_navigate","arguments":{"url":url}}}),
                    );
                    continue;
                }
                if let Some(url) = value
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("browser fixture "))
                {
                    emit(
                        json!({"id":201,"method":"item/tool/call","params":{"tool":"browser_navigate","arguments":{"url":url}}}),
                    );
                    continue;
                }
                if value.to_string().contains("public browser fixture") {
                    emit(
                        json!({"id":401,"method":"item/tool/call","params":{"tool":"browser_navigate","arguments":{"url":"https://www.rust-lang.org/"}}}),
                    );
                    continue;
                }
                if value.to_string().contains("approval fixture") {
                    emit(
                        json!({"id":100,"method":"item/commandExecution/requestApproval","params":{"command":"fixture action","availableDecisions":["accept","decline"]}}),
                    );
                    continue;
                }
                if value.to_string().contains("unauthorized fixture") {
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"failed","error":{"codexErrorInfo":"unauthorized"}}}}),
                    );
                    continue;
                }
                emit(json!({"method":"item/agentMessage/delta","params":{"delta":"가짜 "}}));
                thread::sleep(Duration::from_secs(1));
                if std::env::var_os("MOMSO_TEST_PROVIDER_FAIL_ONCE").is_some() {
                    let marker = std::path::PathBuf::from(
                        std::env::var_os("MOMSO_SERVER_DATA_DIR").unwrap(),
                    )
                    .join("fake-provider-failed");
                    if std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(marker)
                        .is_ok()
                    {
                        emit(
                            json!({"method":"turn/completed","params":{"turn":{"status":"failed"}}}),
                        );
                        continue;
                    }
                }
                emit(json!({"method":"item/agentMessage/delta","params":{"delta":"답"}}));
                emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
            }
            Some(301) => {
                if value.pointer("/result/success").and_then(Value::as_bool) != Some(true) {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":format!("skill fixture error: {value}")}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                    continue;
                }
                emit(
                    json!({"id":302,"method":"item/tool/call","params":{"tool":"browser_navigate","arguments":{"url":crm_url}}}),
                );
            }
            Some(302) => {
                let text = value
                    .pointer("/result/contentItems/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !text.contains("가짜 수업") {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":format!("browser fixture error: {text}")}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                    continue;
                }
                emit(
                    json!({"id":303,"method":"item/tool/call","params":{"tool":"momso_crm_upsert","arguments":{"records":[{"kind":"class","source":{"service":"fixture","externalId":"class-1","observedAt":"2026-11-21T09:00:00+09:00"},"data":{"name":"가짜 수업"}}]}}}),
                );
            }
            Some(303) | Some(304) | Some(305) => {
                let text = value
                    .pointer("/result/contentItems/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                emit(json!({"method":"item/agentMessage/delta","params":{"delta":text}}));
                emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
            }
            Some(221) => {
                emit(
                    json!({"id":222,"method":"item/tool/call","params":{"tool":"browser_request_saved_login","arguments":{}}}),
                );
            }
            Some(222) => {
                if value.pointer("/result/success").and_then(Value::as_bool) == Some(true) {
                    thread::sleep(Duration::from_millis(300));
                    emit(
                        json!({"id":213,"method":"item/tool/call","params":{"tool":"browser_read","arguments":{}}}),
                    );
                } else {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":"login unavailable"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                }
            }
            Some(211) => {
                thread::sleep(Duration::from_millis(500));
                emit(
                    json!({"id":212,"method":"item/tool/call","params":{"tool":"browser_request_human_login","arguments":{"reason":"login"}}}),
                );
            }
            Some(212) => {
                if value.pointer("/result/success").and_then(Value::as_bool) == Some(true) {
                    thread::sleep(Duration::from_millis(300));
                    emit(
                        json!({"id":213,"method":"item/tool/call","params":{"tool":"browser_read","arguments":{}}}),
                    );
                } else {
                    let detail = value
                        .pointer("/result/contentItems/0/text")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":format!("otp unavailable: {detail}")}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                }
            }
            Some(213) | Some(214) => {
                let text = value
                    .pointer("/result/contentItems/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let page: Value = serde_json::from_str(text).unwrap_or(json!({}));
                let text = page["text"].as_str().unwrap_or("no page");
                emit(json!({"method":"item/agentMessage/delta","params":{"delta":text}}));
                emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
            }
            Some(401) | Some(402) | Some(403) | Some(404) => {
                let id = value["id"].as_u64().unwrap();
                let text = value
                    .pointer("/result/contentItems/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let page: Value = serde_json::from_str(text).unwrap_or(json!({}));
                if value.pointer("/result/success").and_then(Value::as_bool) != Some(true) {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":format!("public browser failed at {id}: {}",page["status"])}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                } else if id == 401 || id == 403 {
                    emit(
                        json!({"id":id + 1,"method":"item/tool/call","params":{"tool":"browser_read","arguments":{}}}),
                    );
                } else if id == 402 {
                    public_reads += 1;
                    let link = page["elements"]
                        .as_array()
                        .and_then(|items| items.iter().find(|e| e["name"] == "Learn"));
                    if let Some(link) = link {
                        emit(
                            json!({"id":403,"method":"item/tool/call","params":{"tool":"browser_click","arguments":{"elementId":link["id"]}}}),
                        );
                    } else if public_reads < 7 {
                        thread::sleep(Duration::from_millis(500));
                        emit(
                            json!({"id":402,"method":"item/tool/call","params":{"tool":"browser_read","arguments":{}}}),
                        );
                    } else {
                        emit(
                            json!({"method":"item/agentMessage/delta","params":{"delta":format!("public browser learn link missing: {page}")}}),
                        );
                        emit(
                            json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                        );
                    }
                } else {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":format!("public browser finished: {}",page["text"].as_str().unwrap_or(""))}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                }
            }
            Some(306) => {
                let status =
                    if value.pointer("/result/success").and_then(Value::as_bool) == Some(true) {
                        "generated registered"
                    } else {
                        "generation failed"
                    };
                emit(json!({"method":"item/agentMessage/delta","params":{"delta":status}}));
                emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
            }
            Some(201) => {
                let text = value
                    .pointer("/result/contentItems/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let page: Value = serde_json::from_str(text).unwrap_or(json!({}));
                if let Some(element) = page["elements"]
                    .as_array()
                    .and_then(|v| v.iter().find(|e| e["name"] == "등록"))
                {
                    emit(
                        json!({"id":202,"method":"item/tool/call","params":{"tool":"browser_click","arguments":{"elementId":element["id"]}}}),
                    );
                } else {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"delta":format!("browser error: {}",page["status"])}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
                    );
                }
            }
            Some(202) => {
                let result =
                    if value.pointer("/result/success").and_then(Value::as_bool) == Some(true) {
                        "게시됨"
                    } else {
                        "거절됨"
                    };
                emit(json!({"method":"item/agentMessage/delta","params":{"delta":result}}));
                emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
            }
            Some(100) => {
                let text = if value.pointer("/result/decision").and_then(Value::as_str)
                    == Some("accept")
                {
                    "승인됨"
                } else {
                    "거절됨"
                };
                emit(json!({"method":"item/agentMessage/delta","params":{"delta":text}}));
                emit(json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}));
            }
            _ => {}
        }
    }
}
