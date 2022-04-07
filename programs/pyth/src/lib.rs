use anchor_lang::prelude::*;
pub mod pc;

use pc::{Price, PriceStatus};
use switchboard_program::{
    get_aggregator, get_aggregator_result, mod_AggregatorState, AggregatorState, RoundResult, SwitchboardAccountType,
};
use bytemuck::{cast_slice_mut, from_bytes_mut, try_cast_slice_mut, Pod, Zeroable};
use borsh::{BorshSerialize, BorshDeserialize};
use quick_protobuf::deserialize_from_slice;
use quick_protobuf::serialize_into_slice;

const PROGRAM_ID: Pubkey = Pubkey::new_from_array(include!(concat!(env!("OUT_DIR"), "/pubkey.rs")));

declare_id!(PROGRAM_ID);

#[program]
pub mod pyth {
    use std::cell::RefMut;
    use std::convert::TryInto;
    use std::ops::Div;
    use switchboard_v2::AggregatorAccountData;
    use switchboard_v2::aggregator::{AggregatorRound, Hash};
    use switchboard_v2::decimal::SwitchboardDecimal;
    use super::*;
    pub fn initialize(ctx: Context<Initialize>, price: i64, expo: i32, conf: u64) -> ProgramResult {
        let oracle = &ctx.accounts.price;

        let mut price_oracle = Price::load(oracle).unwrap();

        price_oracle.agg.status = PriceStatus::Trading;
        price_oracle.agg.price = price;
        price_oracle.agg.conf = conf;
        price_oracle.twap.val = price;
        price_oracle.twac.val = conf.try_into().unwrap();
        price_oracle.expo = expo;
        price_oracle.ptype = pc::PriceType::Price;
        price_oracle.num_qt = 3;
        price_oracle.magic = 0xa1b2c3d4;
        price_oracle.ver = 2;

        let slot = ctx.accounts.clock.slot;
        price_oracle.valid_slot = slot;

        Ok(())
    }

    pub fn initialize_switchboard_v1(ctx: Context<Initialize>, mantissa: i128, scale: u32) -> ProgramResult {
        let mut account_data = ctx.accounts.price.data.borrow_mut();
        account_data[0] = SwitchboardAccountType::TYPE_AGGREGATOR as u8;

        let configs = Some(mod_AggregatorState::Configs{
            min_confirmations: Some(3),
          ..mod_AggregatorState::Configs::default()
        });
        let mantissa_f64 = mantissa as f64;
        let denominator = (10u128.pow(scale)) as f64;
        let price = mantissa_f64.div(denominator);
        let last_round_result = Some(RoundResult{
            num_success: Some(3),
            result: Some(price),
            round_open_slot: Some(0),
            ..RoundResult::default()
        });
        let aggregator_state = AggregatorState{
            last_round_result,
            configs,
            ..AggregatorState::default()
        };
        serialize_into_slice(&aggregator_state, &mut account_data[1..]);
        //let _ = switchboard_program::get_aggregator(&ctx.accounts.price).unwrap();

        Ok(())
    }


    pub fn initialize_switchboard_v2(ctx: Context<Initialize>, mantissa: i128, scale: u32) -> ProgramResult {
        let mut account_data = ctx.accounts.price.data.borrow_mut();
        let discriminator: [u8;8] = [217, 230, 65, 101, 201, 162, 27, 125];
        &account_data[..8].copy_from_slice(&discriminator);
        let aggregator_account_data : &mut AggregatorAccountData = bytemuck::from_bytes_mut(&mut account_data[8..]);
        aggregator_account_data.latest_confirmed_round.result = SwitchboardDecimal::new(mantissa, scale);
        aggregator_account_data.latest_confirmed_round.num_success = 3;
        aggregator_account_data.min_oracle_results = 3;
        Ok(())
    }



    pub fn set_price(ctx: Context<SetPrice>, price: i64) -> ProgramResult {
        let oracle = &ctx.accounts.price;

        let mut price_oracle = Price::load(oracle).unwrap();
        price_oracle.agg.price = price;

        let slot = ctx.accounts.clock.slot;
        price_oracle.valid_slot = slot;
        msg!("Price {} updated to {} at slot {}", oracle.key, price, slot);
        Ok(())
    }

    pub fn set_price_switchboard_v1(ctx: Context<SetPrice>, mantissa: i128, scale: u32) -> ProgramResult {
        let mut state_buffer = ctx.accounts.price.try_borrow_mut_data()?;
        let mut aggregator_state: AggregatorState =
            deserialize_from_slice(&state_buffer[1..]).map_err(|_| ProgramError::InvalidAccountData)?;
        let mut last_round_result = aggregator_state.last_round_result.unwrap();
        let price: f64 = mantissa.div(10i128.pow(scale)) as f64;
        last_round_result.result = Some(price);
        aggregator_state.last_round_result = Some(last_round_result);

        let vector = aggregator_state.try_to_vec().unwrap();
        state_buffer[1..].copy_from_slice(&vector);

        Ok(())
    }


    pub fn set_trading(ctx: Context<SetPrice>, status: u8) -> ProgramResult {
        let oracle = &ctx.accounts.price;
        let mut price_oracle = Price::load(oracle).unwrap();
        match status {
            0 => price_oracle.agg.status = PriceStatus::Unknown,
            1 => price_oracle.agg.status = PriceStatus::Trading,
            2 => price_oracle.agg.status = PriceStatus::Halted,
            3 => price_oracle.agg.status = PriceStatus::Auction,
            _ => {
                msg!("Unknown status: {}", status);
                return Err(ProgramError::Custom(1559));
            }
        }
        Ok(())
    }
    pub fn set_twap(ctx: Context<SetPrice>, value: u64) -> ProgramResult {
        let oracle = &ctx.accounts.price;
        let mut price_oracle = Price::load(oracle).unwrap();
        price_oracle.twap.val = value.try_into().unwrap();

        Ok(())
    }
    pub fn set_confidence(ctx: Context<SetPrice>, value: u64) -> ProgramResult {
        let oracle = &ctx.accounts.price;
        let mut price_oracle = Price::load(oracle).unwrap();
        price_oracle.agg.conf = value;

        Ok(())
    }
}
#[derive(Accounts)]
pub struct SetPrice<'info> {
    /// CHECK: Not safe but this is a test tool
    #[account(mut)]
    pub price: AccountInfo<'info>,
    pub clock: Sysvar<'info, Clock>,
}
#[derive(Accounts)]
pub struct Initialize<'info> {
    /// CHECK: Not safe but this is a test tool
    #[account(mut)]
    pub price: AccountInfo<'info>,
    pub clock: Sysvar<'info, Clock>,
}

