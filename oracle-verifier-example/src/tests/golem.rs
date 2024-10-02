use lch_orch::construct_golem;

#[test]
fn happy_path_works() {
    let chain = construct_golem();
    super::common::happy_path(chain);
}

#[test]
fn threshold_not_met() {
    let chain = construct_golem();
    super::common::threshold_not_met(chain);
}
