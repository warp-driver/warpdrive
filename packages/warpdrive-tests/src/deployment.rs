use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use warpdrive_types::{ChainAddress, Service, Workflow, WorkflowId};

#[derive(Clone, Debug)]
pub struct ServiceDeployment {
    pub service: Service,
    pub submission_handlers: BTreeMap<WorkflowId, ChainAddress>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WorkflowDeployment {
    pub workflow: Workflow,
    pub submission_handler: ChainAddress,
}
