use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Mint, Transfer, MintTo};
use solana_program::hash::{hash, Hash};

declare_id!("2BYNDrj1KLe4DiKwu9UicZP5GzYJbM7fY13eYmdCF9pG");

#[program]
pub mod otcl {
    use super::*;

    // Constants for fee calculations and VIP discount.
    const FEE_PERCENTAGE: u64 = 1; // 1% fee on fills.
    const DISCOUNT_THRESHOLD: u64 = 1000; // Minimum staked tokens to get VIP discount.
    const VIP_DISCOUNT_MULTIPLIER: u64 = 50; // 50% discount on fee.

    /// Create a new OTC order by locking collateral.
    /// The caller provides a TTL (time-to-live) which is added to the current time.
    /// Optionally, the order may be multisig (requiring additional approvals).
    pub fn create_order(
        ctx: Context<CreateOrder>,
        price: u64,
        quantity: u64,
        ttl: i64,
        is_multisig: bool,
        multisig_threshold: u8,
    ) -> Result<()> {
        let order = &mut ctx.accounts.order;
        let current_time = Clock::get()?.unix_timestamp;
        order.trader = ctx.accounts.trader.key();
        order.price = price;
        order.quantity = quantity;
        order.remaining_quantity = quantity;
        order.status = OrderStatus::Open;
        order.created_at = current_time;
        order.expiration_at = current_time.checked_add(ttl).unwrap();
        order.is_multisig = is_multisig;
        order.multisig_threshold = multisig_threshold;
        order.approvals = 0;
        order.priority = 0; // can be updated via stake tier logic.
        order.commit_hash = [0; 32]; // initially empty.

        // Transfer collateral from trader's token account to the vault.
        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = Transfer {
                from: ctx.accounts.trader_token_account.to_account_info().clone(),
                to: ctx.accounts.vault_token_account.to_account_info().clone(),
                authority: ctx.accounts.trader.to_account_info().clone(),
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::transfer(cpi_ctx, quantity)?;
        }
        Ok(())
    }

    /// Cancel an open order and return any remaining collateral.
    pub fn cancel_order(ctx: Context<CancelOrder>) -> Result<()> {
        let order = &mut ctx.accounts.order;
        require!(order.trader == ctx.accounts.trader.key(), ErrorCode::Unauthorized);
        require!(order.status == OrderStatus::Open, ErrorCode::OrderNotOpen);

        // Save remaining amount before doing CPI.
        let amount = order.remaining_quantity;
        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = Transfer {
                from: ctx.accounts.vault_token_account.to_account_info().clone(),
                to: ctx.accounts.trader_token_account.to_account_info().clone(),
                // In production, replace this placeholder with a PDA-derived authority.
                authority: order.to_account_info().clone(),
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::transfer(cpi_ctx, amount)?;
        }
        order.status = OrderStatus::Cancelled;
        Ok(())
    }

    /// Expire an order if the current time has surpassed its expiration timestamp.
    pub fn expire_order(ctx: Context<ExpireOrder>) -> Result<()> {
        let order = &mut ctx.accounts.order;
        let current_time = Clock::get()?.unix_timestamp;
        require!(current_time >= order.expiration_at, ErrorCode::OrderNotExpired);

        let amount = order.remaining_quantity;
        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = Transfer {
                from: ctx.accounts.vault_token_account.to_account_info().clone(),
                to: ctx.accounts.trader_token_account.to_account_info().clone(),
                authority: order.to_account_info().clone(), // Placeholder; use PDA in production.
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::transfer(cpi_ctx, amount)?;
        }
        order.status = OrderStatus::Expired;
        Ok(())
    }

    /// Approve a multisig order. Each valid signer calls this to increment the approval counter.
    pub fn approve_order(ctx: Context<ApproveOrder>) -> Result<()> {
        let order = &mut ctx.accounts.order;
        let multisig = &ctx.accounts.multisig;
        require!(order.is_multisig, ErrorCode::NotMultisigOrder);
        // Verify that the approver is one of the multisig owners.
        require!(
            multisig.owners.contains(&ctx.accounts.approver.key()),
            ErrorCode::Unauthorized
        );
        // For simplicity, assume each owner calls only once.
        order.approvals = order.approvals.checked_add(1).unwrap();
        Ok(())
    }

    /// Fill (execute) a portion or the entirety of an open order.
    /// A fee is deducted (with VIP discount if applicable) and collected in the treasury.
    /// The market maker is rewarded by minting OTCL tokens.
    pub fn fill_order(ctx: Context<FillOrder>, fill_quantity: u64) -> Result<()> {
        let order = &mut ctx.accounts.order;
        require!(order.status == OrderStatus::Open, ErrorCode::OrderNotOpen);
        require!(fill_quantity <= order.remaining_quantity, ErrorCode::InvalidFillQuantity);

        // Ensure order has not expired.
        let current_time = Clock::get()?.unix_timestamp;
        require!(current_time < order.expiration_at, ErrorCode::OrderExpired);

        // Calculate fee.
        let mut fee = fill_quantity.checked_mul(FEE_PERCENTAGE).unwrap() / 100;
        if ctx.accounts.market_maker_stake.amount >= DISCOUNT_THRESHOLD {
            fee = fee.checked_mul(VIP_DISCOUNT_MULTIPLIER).unwrap() / 100;
        }
        let net_fill = fill_quantity.checked_sub(fee).unwrap();

        order.remaining_quantity = order.remaining_quantity.checked_sub(fill_quantity).unwrap();
        if order.remaining_quantity == 0 {
            order.status = OrderStatus::Filled;
        }

        // Transfer the net fill (after fee) to the market maker.
        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = Transfer {
                from: ctx.accounts.vault_token_account.to_account_info().clone(),
                to: ctx.accounts.market_maker_token_account.to_account_info().clone(),
                authority: order.to_account_info().clone(), // Placeholder; use PDA in production.
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::transfer(cpi_ctx, net_fill)?;
        }

        // Add fee to the treasury.
        ctx.accounts.treasury.total_fees = ctx
            .accounts
            .treasury
            .total_fees
            .checked_add(fee)
            .unwrap();

        // Reward the market maker by minting OTCL tokens.
        let reward_amount = calculate_reward(fill_quantity);
        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = MintTo {
                mint: ctx.accounts.reward_mint.to_account_info().clone(),
                to: ctx.accounts.market_maker_token_account.to_account_info().clone(),
                authority: ctx.accounts.reward_mint_authority.clone(),
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::mint_to(cpi_ctx, reward_amount)?;
        }
        Ok(())
    }

    /// Stake OTCL tokens to obtain fee discounts and a VIP priority tier.
    pub fn stake_tokens(ctx: Context<StakeTokens>, amount: u64) -> Result<()> {
        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = Transfer {
                from: ctx.accounts.trader_token_account.to_account_info().clone(),
                to: ctx.accounts.staking_vault.to_account_info().clone(),
                authority: ctx.accounts.trader.to_account_info().clone(),
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::transfer(cpi_ctx, amount)?;
        }
        let stake_account = &mut ctx.accounts.stake_account;
        stake_account.trader = ctx.accounts.trader.key();
        stake_account.amount = stake_account.amount.checked_add(amount).unwrap();
        stake_account.last_updated = Clock::get()?.unix_timestamp;
        stake_account.vip_tier = compute_vip_tier(stake_account.amount);
        Ok(())
    }

    /// Withdraw staked tokens.
    pub fn withdraw_stake(ctx: Context<WithdrawStake>, amount: u64) -> Result<()> {
        let stake_account = &mut ctx.accounts.stake_account;
        require!(stake_account.amount >= amount, ErrorCode::InsufficientStake);

        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = Transfer {
                from: ctx.accounts.staking_vault.to_account_info().clone(),
                to: ctx.accounts.trader_token_account.to_account_info().clone(),
                authority: stake_account.to_account_info().clone(), // Placeholder; use PDA in production.
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::transfer(cpi_ctx, amount)?;
        }
        stake_account.amount = stake_account.amount.checked_sub(amount).unwrap();
        stake_account.last_updated = Clock::get()?.unix_timestamp;
        stake_account.vip_tier = compute_vip_tier(stake_account.amount);
        Ok(())
    }

    /// Commit an order by storing a hash of its details.
    pub fn commit_order(ctx: Context<CommitOrder>, commit_hash: [u8; 32]) -> Result<()> {
        let order = &mut ctx.accounts.order;
        require!(order.commit_hash == [0; 32], ErrorCode::AlreadyCommitted);
        order.commit_hash = commit_hash;
        Ok(())
    }

    /// Reveal an order's details, verifying them against the committed hash.
    pub fn reveal_order(
        ctx: Context<RevealOrder>,
        price: u64,
        quantity: u64,
        ttl: i64,
        is_multisig: bool,
        multisig_threshold: u8,
    ) -> Result<()> {
        let order = &mut ctx.accounts.order;
        let data = OrderRevealData {
            price,
            quantity,
            ttl,
            is_multisig,
            multisig_threshold,
        };
        let computed_hash = hash(&data.try_to_vec().unwrap()).to_bytes();
        require!(computed_hash == order.commit_hash, ErrorCode::InvalidReveal);

        order.price = price;
        order.quantity = quantity;
        order.remaining_quantity = quantity;
        let current_time = Clock::get()?.unix_timestamp;
        order.created_at = current_time;
        order.expiration_at = current_time.checked_add(ttl).unwrap();
        order.is_multisig = is_multisig;
        order.multisig_threshold = multisig_threshold;
        Ok(())
    }

    /// Withdraw treasury fees. Intended for governance (DAO/multisig) controlled spending.
    pub fn withdraw_treasury(ctx: Context<WithdrawTreasury>, amount: u64) -> Result<()> {
        let treasury = &mut ctx.accounts.treasury;
        require!(treasury.total_fees >= amount, ErrorCode::InsufficientTreasury);
        treasury.total_fees = treasury.total_fees.checked_sub(amount).unwrap();

        {
            let token_program = &ctx.accounts.token_program;
            let cpi_accounts = Transfer {
                from: ctx.accounts.treasury.to_account_info().clone(),
                to: ctx.accounts.governance_token_account.to_account_info().clone(),
                // In production, the treasury authority should be a PDA.
                authority: ctx.accounts.treasury.to_account_info().clone(),
            };
            let cpi_ctx = CpiContext::new(token_program.to_account_info().clone(), cpi_accounts);
            token::transfer(cpi_ctx, amount)?;
        }
        Ok(())
    }
}

/// Reward logic example: reward 1 OTCL token per 100 units filled.
fn calculate_reward(fill_quantity: u64) -> u64 {
    fill_quantity / 100
}

/// Compute a VIP tier based on staked token amount.
fn compute_vip_tier(amount: u64) -> u8 {
    if amount >= 5000 {
        3
    } else if amount >= 1000 {
        2
    } else if amount > 0 {
        1
    } else {
        0
    }
}

/// Data structure used in the commit–reveal scheme.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct OrderRevealData {
    pub price: u64,
    pub quantity: u64,
    pub ttl: i64,
    pub is_multisig: bool,
    pub multisig_threshold: u8,
}

/// ---
///
/// **Accounts Definitions & Helpers**
///

#[derive(Accounts)]
pub struct CreateOrder<'info> {
    #[account(init, payer = trader, space = 8 + Order::LEN)]
    pub order: Account<'info, Order>,
    #[account(mut)]
    pub trader: Signer<'info>,
    /// The trader’s token account holding collateral.
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,
    /// The vault token account to hold locked collateral.
    #[account(mut)]
    pub vault_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CancelOrder<'info> {
    #[account(mut, has_one = trader)]
    pub order: Account<'info, Order>,
    pub trader: Signer<'info>,
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub vault_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ExpireOrder<'info> {
    #[account(mut, has_one = trader)]
    pub order: Account<'info, Order>,
    pub trader: Signer<'info>,
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub vault_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ApproveOrder<'info> {
    #[account(mut)]
    pub order: Account<'info, Order>,
    /// The multisig account associated with the order.
    pub multisig: Account<'info, MultiSigAccount>,
    /// The signer approving the order.
    pub approver: Signer<'info>,
}

#[derive(Accounts)]
pub struct FillOrder<'info> {
    #[account(mut)]
    pub order: Account<'info, Order>,
    /// CHECK: Market maker (liquidity provider) signer.
    pub market_maker: Signer<'info>,
    #[account(mut)]
    pub vault_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub market_maker_token_account: Account<'info, TokenAccount>,
    /// The OTCL reward token mint.
    #[account(mut)]
    pub reward_mint: Account<'info, Mint>,
    /// PDA authority for minting rewards.
    pub reward_mint_authority: AccountInfo<'info>,
    /// Market maker's stake account used for VIP discount.
    #[account(mut, seeds = [b"stake", market_maker.key().as_ref()], bump)]
    pub market_maker_stake: Account<'info, StakeAccount>,
    /// Treasury account to collect fees.
    #[account(mut)]
    pub treasury: Account<'info, Treasury>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct StakeTokens<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,
    /// The vault holding staked tokens.
    #[account(mut)]
    pub staking_vault: Account<'info, TokenAccount>,
    /// The stake account tracking staking info.
    #[account(init_if_needed, payer = trader, space = 8 + StakeAccount::LEN, seeds = [b"stake", trader.key().as_ref()], bump)]
    pub stake_account: Account<'info, StakeAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct WithdrawStake<'info> {
    #[account(mut)]
    pub trader: Signer<'info>,
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub staking_vault: Account<'info, TokenAccount>,
    #[account(mut, seeds = [b"stake", trader.key().as_ref()], bump)]
    pub stake_account: Account<'info, StakeAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct CommitOrder<'info> {
    #[account(mut)]
    pub order: Account<'info, Order>,
    pub trader: Signer<'info>,
}

#[derive(Accounts)]
pub struct RevealOrder<'info> {
    #[account(mut, has_one = trader)]
    pub order: Account<'info, Order>,
    pub trader: Signer<'info>,
}

#[derive(Accounts)]
pub struct WithdrawTreasury<'info> {
    #[account(mut)]
    pub treasury: Account<'info, Treasury>,
    #[account(mut)]
    pub governance_token_account: Account<'info, TokenAccount>,
    /// CHECK: Governance authority (e.g. a multisig or DAO-controlled signer).
    pub governance: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

/// ---
///
/// **Data Structures**
///

#[account]
pub struct Order {
    pub trader: Pubkey,
    pub price: u64,
    pub quantity: u64,
    pub remaining_quantity: u64,
    pub status: OrderStatus,
    pub created_at: i64,
    pub expiration_at: i64,
    pub is_multisig: bool,
    pub multisig_threshold: u8,
    pub approvals: u8,
    pub priority: u8,
    pub commit_hash: [u8; 32],
}

impl Order {
    const LEN: usize = 32  // trader
        + 8   // price
        + 8   // quantity
        + 8   // remaining_quantity
        + 1   // status (enum as u8)
        + 8   // created_at
        + 8   // expiration_at
        + 1   // is_multisig
        + 1   // multisig_threshold
        + 1   // approvals
        + 1   // priority
        + 32; // commit_hash
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum OrderStatus {
    Open,
    Filled,
    Cancelled,
    Expired,
}

#[account]
pub struct StakeAccount {
    pub trader: Pubkey,
    pub amount: u64,
    pub last_updated: i64,
    pub vip_tier: u8,
}

impl StakeAccount {
    const LEN: usize = 32  // trader
        + 8   // amount
        + 8   // last_updated
        + 1;  // vip_tier
}

#[account]
pub struct Treasury {
    pub total_fees: u64,
}

impl Treasury {
    const LEN: usize = 8; // total_fees.
}

#[account]
pub struct MultiSigAccount {
    pub owners: Vec<Pubkey>,
    pub threshold: u8,
}

impl MultiSigAccount {
    // Allocate space for up to 10 owners.
    const LEN: usize = 4 + (10 * 32) + 1;
}
 
/// ---
///
/// **Custom Errors**
///
#[error_code]
pub enum ErrorCode {
    #[msg("Unauthorized action.")]
    Unauthorized,
    #[msg("Order is not open.")]
    OrderNotOpen,
    #[msg("Invalid fill quantity.")]
    InvalidFillQuantity,
    #[msg("Insufficient staked tokens.")]
    InsufficientStake,
    #[msg("Order has not expired yet.")]
    OrderNotExpired,
    #[msg("Order is expired.")]
    OrderExpired,
    #[msg("Not a multisig order.")]
    NotMultisigOrder,
    #[msg("Invalid reveal data.")]
    InvalidReveal,
    #[msg("Order already committed.")]
    AlreadyCommitted,
    #[msg("Insufficient treasury funds.")]
    InsufficientTreasury,
}
