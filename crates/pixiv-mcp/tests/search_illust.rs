use pixiv_mcp::{search_illust_tool, stdio::serve};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Deserialize)]
struct Contract {
    tool: Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    arguments: Value,
    bodies: Vec<Value>,
    result: Value,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    rpc_error: String,
}
struct Fixture<'a> {
    bodies: &'a [Value],
    seen: &'a Mutex<Vec<BTreeMap<String, Vec<String>>>>,
}
impl Transport for Fixture<'_> {
    async fn send(&self, request: Request) -> Result<Response> {
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = match query
            .get("offset")
            .and_then(|values| values.first())
            .map(String::as_str)
        {
            Some("30") => 1,
            Some("60") => 2,
            _ => 0,
        };
        self.seen.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies.get(index).unwrap_or(&self.bodies[0]).clone(),
        })
    }
}
#[tokio::test]
async fn search_tool_matches_go_schema_logical_pages_filters_and_bookmark_strategies() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-search.json"
    ))
    .unwrap();
    assert_eq!(search_illust_tool(), contract.tool);
    assert_eq!(contract.cases.len(), 144);
    for case in contract.cases {
        let seen = Mutex::new(vec![]);
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                bodies: &case.bodies,
                seen: &seen,
            },
        );
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
        let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search_illust","arguments":case.arguments}});
        let input = init.to_string() + "\n" + &call.to_string() + "\n";
        let mut output = vec![];
        serve(&client, input.as_bytes(), &mut output).await.unwrap();
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let actual = responses
            .iter()
            .find(|response| response["id"] == 7)
            .unwrap();
        if case.rpc_error.is_empty() {
            assert_eq!(actual["result"], case.result, "{}", case.name);
        } else {
            assert_eq!(actual["error"]["code"], -32602, "{}", case.name);
            assert_eq!(
                actual["error"]["message"],
                case.rpc_error
                    .strip_prefix("calling \"tools/call\": ")
                    .unwrap(),
                "{}",
                case.name
            );
        }
        assert_eq!(
            *seen.lock().unwrap(),
            case.queries,
            "{} requests",
            case.name
        );
    }
}
