use anchor_lang::prelude::*;
use solana_program::borsh0_10::try_from_slice_unchecked;

use crate::{utils::hours_since_timestamp, DatedPrice, Price, Result, ScopeError};

use self::spl_stake_pool::StakePool;

const DECIMALS: u32 = 15u32;

// Gives the price of 1 staked SOL in SOL
pub fn get_price(
    stake_pool_account_info: &AccountInfo,
    current_clock: &Clock,
) -> Result<DatedPrice> {
    let stake_pool = try_from_slice_unchecked::<StakePool>(&stake_pool_account_info.data.borrow())
        .map_err(|_| {
            msg!("Provided pubkey is not a SPL Stake account");
            ScopeError::UnexpectedAccount
        })?;

    #[cfg(not(feature = "skip_price_validation"))]
    {
        let hours_since_epoch_started = hours_since_timestamp(
            current_clock.unix_timestamp as u64,
            current_clock.epoch_start_timestamp as u64,
        );
        if stake_pool.last_update_epoch != current_clock.epoch && hours_since_epoch_started >= 1 {
            // The price has not been refreshed this epoch and it's been 1 hour
            msg!("SPL Stake account has not been refreshed in current epoch");
            #[cfg(not(feature = "localnet"))]
            return Err(ScopeError::PriceNotValid.into());
        }
    }

    let value = scaled_rate(&stake_pool)?;

    let price = Price {
        value,
        exp: DECIMALS.into(),
    };
    let dated_price = DatedPrice {
        price,
        last_updated_slot: current_clock.slot,
        unix_timestamp: u64::try_from(current_clock.unix_timestamp).unwrap(),
        ..Default::default()
    };

    Ok(dated_price)
}

fn scaled_rate(stake_pool: &StakePool) -> Result<u64> {
    const FACTOR: u64 = 10u64.pow(DECIMALS);
    stake_pool
        .calc_lamports_withdraw_amount(FACTOR)
        .ok_or_else(|| ScopeError::MathOverflow.into())
}

mod spl_stake_pool {
    use anchor_lang::prelude::borsh::BorshSchema;
    use solana_program::stake::state::Lockup;

    use super::*;

    /// Wrapper type that "counts down" epochs, which is Borsh-compatible with the
    /// native `Option`
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, AnchorSerialize, AnchorDeserialize, BorshSchema)]
    pub(crate) enum FutureEpoch<T> {
        /// Nothing is set
        None,
        /// Value is ready after the next epoch boundary
        One(T),
        /// Value is ready after two epoch boundaries
        Two(T),
    }

    impl<T> Default for FutureEpoch<T> {
        fn default() -> Self {
            Self::None
        }
    }

    /// Enum representing the account type managed by the program
    #[derive(Clone, Debug, Default, PartialEq, AnchorDeserialize, AnchorSerialize, BorshSchema)]
    pub(crate) enum AccountType {
        /// If the account has not been initialized, the enum will be 0
        #[default]
        Uninitialized,
        /// Stake pool
        StakePool,
        /// Validator stake list
        ValidatorList,
    }

    #[repr(C)]
    #[derive(
        Clone, Copy, Debug, Default, PartialEq, AnchorSerialize, AnchorDeserialize, BorshSchema,
    )]
    pub(crate) struct Fee {
        /// denominator of the fee ratio
        pub denominator: u64,
        /// numerator of the fee ratio
        pub numerator: u64,
    }

    /// The type of fees that can be set on the stake pool
    #[derive(Clone, Debug, PartialEq, AnchorDeserialize, AnchorSerialize, BorshSchema)]
    pub(crate) enum FeeType {
        /// Referral fees for SOL deposits
        SolReferral(u8),
        /// Referral fees for stake deposits
        StakeReferral(u8),
        /// Management fee paid per epoch
        Epoch(Fee),
        /// Stake withdrawal fee
        StakeWithdrawal(Fee),
        /// Deposit fee for SOL deposits
        SolDeposit(Fee),
        /// Deposit fee for stake deposits
        StakeDeposit(Fee),
        /// SOL withdrawal fee
        SolWithdrawal(Fee),
    }

    /// Initialized program details.
    #[repr(C)]
    #[derive(Clone, Debug, Default, PartialEq, AnchorDeserialize, AnchorSerialize, BorshSchema)]
    pub(crate) struct StakePool {
        /// Account type, must be StakePool currently
        pub account_type: AccountType,

        /// Manager authority, allows for updating the staker, manager, and fee account
        pub manager: Pubkey,

        /// Staker authority, allows for adding and removing validators, and managing stake
        /// distribution
        pub staker: Pubkey,

        /// Stake deposit authority
        ///
        /// If a depositor pubkey is specified on initialization, then deposits must be
        /// signed by this authority. If no deposit authority is specified,
        /// then the stake pool will default to the result of:
        /// `Pubkey::find_program_address(
        ///     &[&stake_pool_address.as_ref(), b"deposit"],
        ///     program_id,
        /// )`
        pub stake_deposit_authority: Pubkey,

        /// Stake withdrawal authority bump seed
        /// for `create_program_address(&[state::StakePool account, "withdrawal"])`
        pub stake_withdraw_bump_seed: u8,

        /// Validator stake list storage account
        pub validator_list: Pubkey,

        /// Reserve stake account, holds deactivated stake
        pub reserve_stake: Pubkey,

        /// Pool Mint
        pub pool_mint: Pubkey,

        /// Manager fee account
        pub manager_fee_account: Pubkey,

        /// Pool token program id
        pub token_program_id: Pubkey,

        /// Total stake under management.
        /// Note that if `last_update_epoch` does not match the current epoch then
        /// this field may not be accurate
        pub total_lamports: u64,

        /// Total supply of pool tokens (should always match the supply in the Pool Mint)
        pub pool_token_supply: u64,

        /// Last epoch the `total_lamports` field was updated
        pub last_update_epoch: u64,

        /// Lockup that all stakes in the pool must have
        pub lockup: Lockup,

        /// Fee taken as a proportion of rewards each epoch
        pub epoch_fee: Fee,

        /// Fee for next epoch
        pub next_epoch_fee: FutureEpoch<Fee>,

        /// Preferred deposit validator vote account pubkey
        pub preferred_deposit_validator_vote_address: Option<Pubkey>,

        /// Preferred withdraw validator vote account pubkey
        pub preferred_withdraw_validator_vote_address: Option<Pubkey>,

        /// Fee assessed on stake deposits
        pub stake_deposit_fee: Fee,

        /// Fee assessed on withdrawals
        pub stake_withdrawal_fee: Fee,

        /// Future stake withdrawal fee, to be set for the following epoch
        pub next_stake_withdrawal_fee: FutureEpoch<Fee>,

        /// Fees paid out to referrers on referred stake deposits.
        /// Expressed as a percentage (0 - 100) of deposit fees.
        /// i.e. `stake_deposit_fee`% of stake deposited is collected as deposit fees for every deposit
        /// and `stake_referral_fee`% of the collected stake deposit fees is paid out to the referrer
        pub stake_referral_fee: u8,

        /// Toggles whether the `DepositSol` instruction requires a signature from
        /// this `sol_deposit_authority`
        pub sol_deposit_authority: Option<Pubkey>,

        /// Fee assessed on SOL deposits
        pub sol_deposit_fee: Fee,

        /// Fees paid out to referrers on referred SOL deposits.
        /// Expressed as a percentage (0 - 100) of SOL deposit fees.
        /// i.e. `sol_deposit_fee`% of SOL deposited is collected as deposit fees for every deposit
        /// and `sol_referral_fee`% of the collected SOL deposit fees is paid out to the referrer
        pub sol_referral_fee: u8,

        /// Toggles whether the `WithdrawSol` instruction requires a signature from
        /// the `deposit_authority`
        pub sol_withdraw_authority: Option<Pubkey>,

        /// Fee assessed on SOL withdrawals
        pub sol_withdrawal_fee: Fee,

        /// Future SOL withdrawal fee, to be set for the following epoch
        pub next_sol_withdrawal_fee: FutureEpoch<Fee>,

        /// Last epoch's total pool tokens, used only for APR estimation
        pub last_epoch_pool_token_supply: u64,

        /// Last epoch's total lamports, used only for APR estimation
        pub last_epoch_total_lamports: u64,
    }

    impl StakePool {
        /// calculate lamports amount on withdrawal
        #[inline]
        pub fn calc_lamports_withdraw_amount(&self, pool_tokens: u64) -> Option<u64> {
            // `checked_div` returns `None` for a 0 quotient result, but in this
            // case, a return of 0 is valid for small amounts of pool tokens. So
            // we check for that separately
            let numerator = (pool_tokens as u128).checked_mul(self.total_lamports as u128)?;
            let denominator = self.pool_token_supply as u128;
            if numerator < denominator || denominator == 0 {
                Some(0)
            } else {
                u64::try_from(numerator.checked_div(denominator)?).ok()
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::oracles::spl_stake::spl_stake_pool::StakePool;

    use super::*;

    #[test]
    pub fn minted_token_is_equal_to_token_in_vault() {
        let total_lamports = 10u64.pow(5);
        let pool_token_supply = 10u64.pow(5);
        let stake_pool = StakePool {
            total_lamports,
            pool_token_supply,
            ..Default::default()
        };
        assert_eq!(scaled_rate(&stake_pool).unwrap(), 10u64.pow(DECIMALS));
    }

    #[test]
    pub fn minted_token_is_2x_token_in_vault() {
        // Note: this should never happen
        let total_lamports = 10u64.pow(5);
        let pool_token_supply = 2 * 10u64.pow(5);
        let stake_pool = StakePool {
            total_lamports,
            pool_token_supply,
            ..Default::default()
        };
        // Expect staked token price to be 0.5 token
        assert_eq!(
            scaled_rate(&stake_pool).unwrap(),
            5 * 10u64.pow(DECIMALS - 1)
        );
    }

    #[test]
    pub fn token_in_vault_is_2x_token_minted() {
        let total_lamports = 2 * 10u64.pow(5);
        let pool_token_supply = 10u64.pow(5);
        let stake_pool = StakePool {
            total_lamports,
            pool_token_supply,
            ..Default::default()
        };
        // Expect staked token price to be 2 tokens
        assert_eq!(scaled_rate(&stake_pool).unwrap(), 2 * 10u64.pow(DECIMALS));
    }
}
