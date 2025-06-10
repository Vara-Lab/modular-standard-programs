
#![no_std]

use sails_rs::prelude::*;
use session_service::*; 

pub mod services;

use services::service::{Service, ActionsForSession};

// ⚠️ Generates SessionService, SessionData, Storage, etc. 
session_service::generate_session_system!(ActionsForSession);

pub struct Program;

#[program]
impl Program {
    /// Constructor for Lending contract.
    /// `collateral_token` - address of the VFT used as collateral.
    /// `debt_token` - address of the VFT contract used as borrow (debt) asset.
    /// `base_interest_rate` - annual interest rate in 1e18 decimals (e.g. 3% = 3_000_000_000_000_000_000).
    /// `min_loan`, `max_loan` - principal limits, in debt token smallest units.
    pub fn new(
        collateral_token: ActorId,
        debt_token: ActorId,
        base_interest_rate: u128,
        min_loan: u128,
        max_loan: u128,
        config: Config,
    ) -> Self {
        Service::seed(collateral_token, debt_token, base_interest_rate, min_loan, max_loan);
        SessionService::init(config);
        Self
    }

    #[export(route = "Service")]
    pub fn lending(&self) -> Service {
        Service::new()
    }

    #[export(route = "Session")]
    pub fn session(&self) -> SessionService {
        SessionService::new()
    }
}
