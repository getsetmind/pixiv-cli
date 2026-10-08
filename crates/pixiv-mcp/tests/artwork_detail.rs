use pixiv_mcp::{IllustReference, illust_detail, illust_detail_tool};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Mutex;

#[derive(Deserialize)]
struct Contract {
    tool: Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    arguments: Value,
    body: Value,
    result: Value,
    calls: usize,
    requests: usize,
    rpc_error: String,
}
struct Fixture<'a> {
    body: Value,
    requests: &'a Mutex<Vec<Request>>,
}
impl Transport for Fixture<'_> {
    async fn send(&self, request: Request) -> Result<Response> {
        self.requests.lock().unwrap().push(request);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}

#[tokio::test]
async fn artwork_detail_matches_go_metadata_reference_validation_and_structured_records() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-detail.json"
    ))
    .unwrap();
    assert_eq!(illust_detail_tool(), contract.tool);
    assert_eq!(contract.cases.len(), 33);
    assert_eq!(
        contract
            .cases
            .iter()
            .filter(|case| !case.rpc_error.is_empty())
            .count(),
        1
    );
    for case in contract
        .cases
        .into_iter()
        .filter(|case| case.rpc_error.is_empty())
    {
        let requests = Mutex::new(vec![]);
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                body: case.body,
                requests: &requests,
            },
        );
        let input: IllustReference = serde_json::from_value(case.arguments).unwrap();
        let result = illust_detail(&client, input).await;
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            case.result,
            "{}",
            case.name
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), case.requests, "{}", case.name);
        assert_eq!(case.calls, case.requests, "{}", case.name);
        for request in requests.iter() {
            assert_eq!(request.method, "GET");
            assert_eq!(request.url, "https://app-api.pixiv.net/v1/illust/detail");
        }
    }
}
