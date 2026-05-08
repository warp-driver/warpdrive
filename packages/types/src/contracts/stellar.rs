use stellar_xdr::curr::ContractId;

#[derive(Debug, Clone)]
pub struct StellarServiceManagerContracts {
    pub verifier: ContractId,
    pub security: ContractId,
}
