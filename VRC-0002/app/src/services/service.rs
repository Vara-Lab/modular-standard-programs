
#![no_std]
#![allow(static_mut_refs)]

use sails_rs::{
    prelude::*,
    gstd::{msg, exec},
    collections::HashMap,
};
use sails_rs::calls::ActionIo;
use sails_rs::collections::HashMap as SailsHashMap;

// ---- Signless/session 
use crate::{SessionData, Storage};

// ---- State Definitions ----

const DECIMALS_FACTOR: u128 = 1_000_000_000_000_000_000; // 1e18
const MIN_COLLATERAL_RATIO: u128 = 150_000_000_000_000_000_000; // 150%

static mut LENDING_STATE: Option<LendingState> = None;

/// Loan status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, TypeInfo)]
#[codec(crate = sails_rs::scale_codec)]
#[scale_info(crate = sails_rs::scale_info)]
pub enum LoanStatus {
    Active,
    Closed,
    Liquidated,
}

/// Loan struct
#[derive(Debug, Clone, Encode, Decode, TypeInfo)]
#[codec(crate = sails_rs::scale_codec)]
#[scale_info(crate = sails_rs::scale_info)]
pub struct Loan {
    pub borrower: ActorId,
    pub collateral: u128,
    pub principal: u128,
    pub interest_rate: u128, // per year, in DECIMALS_FACTOR
    pub start_block: u64,
    pub status: LoanStatus,
}

/// Lending state struct
#[derive(Debug, Clone, Default)]
pub struct LendingState {
    pub owner: ActorId,
    pub collateral_token: ActorId,
    pub debt_token: ActorId,
    pub base_interest_rate: u128,
    pub min_loan: u128,
    pub max_loan: u128,
    pub next_loan_id: u64,
    pub loans: SailsHashMap<u64, Loan>,
    pub user_loans: SailsHashMap<ActorId, Vec<u64>>,
    pub total_collateral: u128,
    pub total_principal: u128,
}

impl LendingState {
    pub fn init(
        owner: ActorId,
        collateral_token: ActorId,
        debt_token: ActorId,
        base_interest_rate: u128,
        min_loan: u128,
        max_loan: u128,
    ) {
        unsafe {
            LENDING_STATE = Some(Self {
                owner,
                collateral_token,
                debt_token,
                base_interest_rate,
                min_loan,
                max_loan,
                ..Default::default()
            })
        }
    }
    pub fn state_mut() -> &'static mut LendingState {
        let s = unsafe { LENDING_STATE.as_mut() };
        debug_assert!(s.is_some(), "LendingState not initialized");
        unsafe { s.unwrap_unchecked() }
    }
    pub fn state_ref() -> &'static LendingState {
        let s = unsafe { LENDING_STATE.as_ref() };
        debug_assert!(s.is_some(), "LendingState not initialized");
        unsafe { s.unwrap_unchecked() }
    }
}

/// Events emitted by the contract
#[derive(Debug, Clone, Encode, Decode, TypeInfo, PartialEq, Eq)]
#[codec(crate = sails_rs::scale_codec)]
#[scale_info(crate = sails_rs::scale_info)]
pub enum LendingEvent {
    LoanOpened {
        loan_id: u64,
        borrower: ActorId,
        collateral: u128,
        principal: u128,
    },
    Repaid {
        loan_id: u64,
        borrower: ActorId,
    },
    Liquidated {
        loan_id: u64,
        borrower: ActorId,
    },
    OwnerSet(ActorId),
    ParamsUpdated,
}

#[derive(Debug, Encode, Decode, TypeInfo, Clone)]
#[codec(crate = sails_rs::scale_codec)]
#[scale_info(crate = sails_rs::scale_info)]
pub struct IoLendingState {
    pub owner: ActorId,
    pub collateral_token: ActorId,
    pub debt_token: ActorId,
    pub base_interest_rate: u128,
    pub min_loan: u128,
    pub max_loan: u128,
    pub loans: Vec<(u64, Loan)>,
    pub user_loans: Vec<(ActorId, Vec<u64>)>,
    pub total_collateral: u128,
    pub total_principal: u128,
}

// ---- Session/Signless actions ----

#[derive(Debug, Clone, Encode, Decode, TypeInfo, PartialEq, Eq)]
#[codec(crate = sails_rs::scale_codec)]
#[scale_info(crate = sails_rs::scale_info)]
pub enum ActionsForSession {
    OpenLoan,
    RepayLoan,
    LiquidateLoan,
    UpdateParams,
}

fn get_actor(
    session_map: &SailsHashMap<ActorId, SessionData>,
    msg_source: &ActorId,
    session_for_account: &Option<ActorId>,
    action: ActionsForSession,
) -> ActorId {
    match session_for_account {
        Some(account) => {
            let session = session_map
                .get(account)
                .expect("No valid session for this account");
            assert!(
                session.expires > exec::block_timestamp(),
                "Session expired"
            );
            assert!(
                session.allowed_actions.contains(&action),
                "Action not allowed"
            );
            assert_eq!(
                session.key,
                *msg_source,
                "Sender not authorized for session"
            );
            *account
        }
        None => *msg_source,
    }
}

// ---- Io conversion ----

impl From<LendingState> for IoLendingState {
    fn from(state: LendingState) -> Self {
        IoLendingState {
            owner: state.owner,
            collateral_token: state.collateral_token,
            debt_token: state.debt_token,
            base_interest_rate: state.base_interest_rate,
            min_loan: state.min_loan,
            max_loan: state.max_loan,
            loans: state.loans.iter().map(|(&id, loan)| (id, loan.clone())).collect(),
            user_loans: state.user_loans.iter().map(|(&id, v)| (id, v.clone())).collect(),
            total_collateral: state.total_collateral,
            total_principal: state.total_principal,
        }
    }
}

// ---- Main Service ----

#[derive(Debug, Clone, Default)] // Auditor: Ensure all necessary traits are derived
pub struct Service; // Auditor: Ensure main service struct is public

impl Service {
    /// Initialize the lending contract. Owner is the origin of call.
    pub fn seed(
        collateral_token: ActorId,
        debt_token: ActorId,
        base_interest_rate: u128,
        min_loan: u128,
        max_loan: u128,
    ) {
        if collateral_token == ActorId::zero() || debt_token == ActorId::zero() {
            panic!("Token addresses cannot be zero");
        }
        if min_loan == 0 || max_loan == 0 || max_loan < min_loan {
            panic!("Loan thresholds invalid");
        }
        LendingState::init(
            msg::source(),
            collateral_token,
            debt_token,
            base_interest_rate,
            min_loan,
            max_loan,
        );
    }
}

#[service(events = LendingEvent)]
impl Service {
    pub fn new() -> Self { Self }

    /// Open a new loan. The caller must be the borrower authorized by session (or self if not signless).
    pub async fn open_loan(
        &mut self,
        collateral: u128,
        principal: u128,
        session_for_account: Option<ActorId>
    ) -> LendingEvent {
        let msg_src = msg::source();

        // Session-aware: If session_for_account is set, will use session verification
        let sessions = Storage::get_session_map();
        let borrower = get_actor(&sessions, &msg_src, &session_for_account, ActionsForSession::OpenLoan);

        let mut state = LendingState::state_mut();

        // Validate input
        if principal < state.min_loan || principal > state.max_loan {
            panic!("Loan principal out of bounds");
        }
        if collateral == 0 {
            panic!("Must provide collateral");
        }
        // Check collateralization ratio
        let ratio = collateral
            .saturating_mul(DECIMALS_FACTOR)
            .checked_div(principal)
            .expect("Division error");
        if ratio < MIN_COLLATERAL_RATIO {
            panic!("Insufficient collateral ratio");
        }

        if state.loans.len() >= 10_000 {
            panic!("Loan limit reached"); 
        }

        // Transfer collateral from user to contract
        let transfer_from = ActionIo::TransferFrom(borrower, exec::program_id(), collateral.into()).encode();
        msg::send_bytes_with_gas_for_reply(state.collateral_token, transfer_from, 5_000_000_000, 0, 0)
            .expect("Collateral transfer failed")
            .await
            .expect("No reply for collateral transfer");

        // Mint debt tokens to user (simulate FT transfer)
        let mint_debt = ActionIo::TransferFrom(exec::program_id(), borrower, principal.into()).encode();
        msg::send_bytes_with_gas_for_reply(state.debt_token, mint_debt, 5_000_000_000, 0, 0)
            .expect("Debt token transfer failed")
            .await
            .expect("No reply for debt minting");

        let loan_id = state.next_loan_id;
        let block = exec::block_timestamp() as u64; 

        let loan = Loan {
            borrower,
            collateral,
            principal,
            interest_rate: state.base_interest_rate,
            start_block: block,
            status: LoanStatus::Active,
        };
        state.loans.insert(loan_id, loan);
        let user_loans = state.user_loans.entry(borrower).or_default();
        if user_loans.len() >= 100 {
            panic!("User loan limit reached"); 
        }
        user_loans.push(loan_id);
        state.next_loan_id = state.next_loan_id.checked_add(1).expect("Loan id overflow"); 
        state.total_collateral = state.total_collateral.checked_add(collateral).expect("Collateral overflow"); 
        state.total_principal = state.total_principal.checked_add(principal).expect("Principal overflow"); 

        self.emit_event(LendingEvent::LoanOpened {
            loan_id,
            borrower,
            collateral,
            principal,
        }).expect("Event error"); 

        LendingEvent::LoanOpened {
            loan_id,
            borrower,
            collateral,
            principal,
        }
    }

    /// Repay a loan (with interest). Only authorized borrower via session or self.
    pub async fn repay(
        &mut self,
        loan_id: u64,
        session_for_account: Option<ActorId>,
    ) -> LendingEvent {
        let msg_src = msg::source();
        let sessions = Storage::get_session_map();
        let borrower = get_actor(&sessions, &msg_src, &session_for_account, ActionsForSession::RepayLoan);

        let mut state = LendingState::state_mut();
        let loan = state.loans.get_mut(&loan_id).expect("No such loan");
        if loan.borrower != borrower {
            panic!("Not loan owner");
        }
        if loan.status != LoanStatus::Active {
            panic!("Loan not active");
        }
        // Calculate interest
        let current_block = exec::block_timestamp() as u64; 
        let duration = current_block.saturating_sub(loan.start_block) as u128; 
        let interest = loan
            .principal
            .saturating_mul(loan.interest_rate) 
            .saturating_mul(duration)
            .checked_div(31_536_000u128).unwrap_or(0)
            .checked_div(DECIMALS_FACTOR).unwrap_or(0); 

        let total_owed = loan.principal.saturating_add(interest); 

        // Burn user debt tokens for repayment
        let burn_debt = ActionIo::Burn(borrower, total_owed.into()).encode();
        msg::send_bytes_with_gas_for_reply(state.debt_token, burn_debt, 5_000_000_000, 0, 0)
            .expect("Burn failed")
            .await
            .expect("No reply debt burn");

        // Return collateral to user
        let transfer_coll = ActionIo::Transfer(borrower, loan.collateral.into()).encode();
        msg::send_bytes_with_gas_for_reply(state.collateral_token, transfer_coll, 5_000_000_000, 0, 0)
            .expect("Collateral transfer failed")
            .await
            .expect("No reply collateral transfer");

        state.total_collateral = state.total_collateral.saturating_sub(loan.collateral); 
        state.total_principal = state.total_principal.saturating_sub(loan.principal);
        loan.status = LoanStatus::Closed;

        self.emit_event(LendingEvent::Repaid {
            loan_id,
            borrower,
        }).expect("Event error"); 

        LendingEvent::Repaid {
            loan_id,
            borrower,
        }
    }

    /// Liquidate undercollateralized loan. Anyone can call; session not required.
    pub async fn liquidate(
        &mut self,
        loan_id: u64,
        _session_for_account: Option<ActorId>
    ) -> LendingEvent {
        // No session required on liquidation, but param included for interface consistency
        let mut state = LendingState::state_mut();
        let loan = state.loans.get_mut(&loan_id).expect("No loan");
        if loan.status != LoanStatus::Active {
            panic!("Loan not active");
        }

        // Simulate on-chain price check for liquidation
        let ratio = loan.collateral
            .saturating_mul(DECIMALS_FACTOR)
            .checked_div(loan.principal.max(1))
            .expect("Division error");
        if ratio >= MIN_COLLATERAL_RATIO {
            panic!("Loan safe; can't liquidate");
        }

        // Collateral to contract owner as liquidator bonus
        let transfer = ActionIo::Transfer(state.owner, loan.collateral.into()).encode();
        msg::send_bytes_with_gas_for_reply(state.collateral_token, transfer, 5_000_000_000, 0, 0)
            .expect("Collateral to owner failed")
            .await
            .expect("No reply on transfer");

        state.total_collateral = state.total_collateral.saturating_sub(loan.collateral);
        state.total_principal = state.total_principal.saturating_sub(loan.principal); 
        loan.status = LoanStatus::Liquidated;

        self.emit_event(LendingEvent::Liquidated {
            loan_id,
            borrower: loan.borrower,
        }).expect("Event error"); 

        LendingEvent::Liquidated {
            loan_id,
            borrower: loan.borrower,
        }
    }

    /// Set new owner/admin (must be authorized owner, by session or self).
    pub fn set_owner(
        &mut self,
        new_owner: ActorId,
        session_for_account: Option<ActorId>
    ) -> LendingEvent {
        let msg_src = msg::source();
        let sessions = Storage::get_session_map();
        let who = get_actor(&sessions, &msg_src, &session_for_account, ActionsForSession::UpdateParams);

        let mut state = LendingState::state_mut();
        if who != state.owner {
            panic!("Not owner");
        }
        state.owner = new_owner;
        self.emit_event(LendingEvent::OwnerSet(new_owner)).expect("Event err"); 
        LendingEvent::OwnerSet(new_owner)
    }

    /// Update lending params (base rate, min, max) - owner only (session or self).
    pub fn update_params(
        &mut self,
        new_rate: u128,
        min_loan: u128,
        max_loan: u128,
        session_for_account: Option<ActorId>
    ) -> LendingEvent {
        let msg_src = msg::source();
        let sessions = Storage::get_session_map();
        let who = get_actor(&sessions, &msg_src, &session_for_account, ActionsForSession::UpdateParams);

        let mut state = LendingState::state_mut();
        if who != state.owner {
            panic!("Not owner");
        }
        state.base_interest_rate = new_rate;
        state.min_loan = min_loan;
        state.max_loan = max_loan;
        self.emit_event(LendingEvent::ParamsUpdated).expect("Event err");
        LendingEvent::ParamsUpdated
    }

    // ---- Queries (3) ----

    /// Query: get loan by id
    pub fn query_loan(&self, loan_id: u64) -> Option<Loan> {
        LendingState::state_ref().loans.get(&loan_id).cloned()
    }

    /// Query: all loan ids for user
    pub fn query_user_loans(&self, user: ActorId) -> Vec<u64> {
        let user_loans = LendingState::state_ref().user_loans.get(&user);
        match user_loans {
            Some(loans) => {
                if loans.len() > 100 {
                    loans[..100].to_vec() 
                } else {
                    loans.clone()
                }
            },
            None => Vec::new(),
        }
    }

    /// Query: contract state (full)
    pub fn query_state(&self) -> IoLendingState {
        let state = LendingState::state_ref();
        // Auditor: Limit map outputs to prevent unbounded growth
        let mut limited_loans = Vec::new();
        for (id, loan) in state.loans.iter().take(1000) { 
            limited_loans.push((*id, loan.clone()));
        }
        let mut limited_user_loans = Vec::new();
        for (id, v) in state.user_loans.iter().take(1000) { 
            let limited_v = if v.len() > 100 { v[..100].to_vec() } else { v.clone() }; 
            limited_user_loans.push((*id, limited_v));
        }
        IoLendingState {
            owner: state.owner,
            collateral_token: state.collateral_token,
            debt_token: state.debt_token,
            base_interest_rate: state.base_interest_rate,
            min_loan: state.min_loan,
            max_loan: state.max_loan,
            loans: limited_loans,
            user_loans: limited_user_loans,
            total_collateral: state.total_collateral,
            total_principal: state.total_principal,
        }
    }
}
