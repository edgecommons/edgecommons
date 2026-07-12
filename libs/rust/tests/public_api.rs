//! External-crate compile checks for component-author API reachability.

use edgecommons::facades::{AppCorrelation, AppFacade, PreparedAppMessage};
use edgecommons::messaging::Message;
use serde_json::json;

fn prepare_through_public_facade(
    facade: &AppFacade,
    request: &Message,
) -> edgecommons::Result<PreparedAppMessage> {
    facade.prepare_correlated(
        "ImageCaptured",
        "camera/captured",
        json!({ "captureId": "cap-1" }),
        AppCorrelation::from(request),
    )
}

fn prelude_types_are_nameable(
    _prepared: Option<edgecommons::prelude::PreparedAppMessage>,
    _correlation: Option<edgecommons::prelude::AppCorrelation>,
    _outcome: Option<edgecommons::prelude::CommandOutcome>,
    _token: Option<edgecommons::prelude::DeferredReplyToken>,
) {
}

#[test]
fn new_component_author_types_are_publicly_reachable() {
    let _prepare: fn(&AppFacade, &Message) -> edgecommons::Result<PreparedAppMessage> =
        prepare_through_public_facade;
    prelude_types_are_nameable(None, None, None, None);
}
