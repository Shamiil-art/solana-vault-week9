use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount, Transfer};

declare_id!("AdDmG6ALyofdymfyk9mx4vzAPFnnYqQYSi6WFmznDkGQ");

/// Комиссия с депозита: 100 bps = 1%.
pub const DEPOSIT_FEE_BPS: u64 = 100;
pub const BPS_DENOMINATOR: u64 = 10_000;
/// Минимальный депозит в целых токенах (1 токен = 10^decimals базовых единиц).
pub const MIN_DEPOSIT_TOKENS: u64 = 1;

#[program]
pub mod simple_vault {
    use super::*;

    pub fn initialize_vault(ctx: Context<InitializeVault>) -> Result<()> {
        let one_token = 10u64
            .checked_pow(ctx.accounts.token_mint.decimals as u32)
            .ok_or(VaultError::MathOverflow)?;

        let vault = &mut ctx.accounts.vault_state;
        vault.admin = ctx.accounts.admin.key();
        vault.token_mint = ctx.accounts.token_mint.key();
        vault.vault_token_account = ctx.accounts.vault_token_account.key();
        vault.share_mint = ctx.accounts.share_mint.key();
        vault.fee_wallet = ctx.accounts.fee_wallet.key();
        vault.total_tokens = 0;
        vault.total_shares = 0;
        vault.min_deposit = MIN_DEPOSIT_TOKENS
            .checked_mul(one_token)
            .ok_or(VaultError::MathOverflow)?;
        vault.bump = ctx.bumps.vault_state;
        vault.paused = false;

        msg!("Vault initialized for mint: {}", vault.token_mint);
        Ok(())
    }

    pub fn create_user_card(ctx: Context<CreateUserCard>) -> Result<()> {
        require!(!ctx.accounts.vault_state.paused, VaultError::VaultPaused);
        let card = &mut ctx.accounts.user_card;
        card.vault = ctx.accounts.vault_state.key();
        card.owner = ctx.accounts.user.key();
        card.bump = ctx.bumps.user_card;
        card.created_at = Clock::get()?.unix_timestamp;

        msg!("User card created for: {}", card.owner);
        Ok(())
    }

    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        ctx.accounts.vault_state.paused = paused;
        Ok(())
    }

    pub fn close_user_card(_ctx: Context<CloseUserCard>) -> Result<()> {
        Ok(())
    }

    /// Пользователь вносит `amount` токенов и получает share-токены.
    ///
    /// 1% от `amount` уходит на `fee_wallet`, остальное (`net`) зачисляется в vault.
    ///   Первый депозит:       shares = net  (1:1)
    ///   Последующие депозиты: shares = net * total_shares / total_tokens
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        let vault = &ctx.accounts.vault_state;
        require!(!vault.paused, VaultError::VaultPaused);
        require!(amount > 0, VaultError::ZeroAmount);
        require!(amount >= vault.min_deposit, VaultError::DepositTooSmall);

        let fee = deposit_fee(amount)?;
        let net = amount.checked_sub(fee).ok_or(VaultError::MathOverflow)?;
        let shares = shares_for_deposit(net, vault.total_tokens, vault.total_shares)?;
        require!(shares > 0, VaultError::SharesTooSmall);

        // 1. Комиссия: пользователь → fee_wallet
        if fee > 0 {
            token::transfer(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.user_token_account.to_account_info(),
                        to: ctx.accounts.fee_wallet.to_account_info(),
                        authority: ctx.accounts.user.to_account_info(),
                    },
                ),
                fee,
            )?;
        }

        // 2. Основная сумма: пользователь → vault
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.user_token_account.to_account_info(),
                    to: ctx.accounts.vault_token_account.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            net,
        )?;

        // 3. Vault PDA печатает shares пользователю
        let token_mint_key = vault.token_mint;
        let seeds = &[b"vault".as_ref(), token_mint_key.as_ref(), &[vault.bump]];
        token::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.share_mint.to_account_info(),
                    to: ctx.accounts.user_share_account.to_account_info(),
                    authority: ctx.accounts.vault_state.to_account_info(),
                },
                &[&seeds[..]],
            ),
            shares,
        )?;

        // 4. Обновляем учёт в vault и в карточке
        let vault = &mut ctx.accounts.vault_state;
        vault.total_tokens = vault.total_tokens.checked_add(net).ok_or(VaultError::MathOverflow)?;
        vault.total_shares = vault.total_shares.checked_add(shares).ok_or(VaultError::MathOverflow)?;

        let card = &mut ctx.accounts.user_card;
        card.deposited = card.deposited.checked_add(net).ok_or(VaultError::MathOverflow)?;
        card.shares = card.shares.checked_add(shares).ok_or(VaultError::MathOverflow)?;

        msg!(
            "Deposit: {} tokens (fee {}) -> {} shares (total_tokens={}, total_shares={})",
            amount, fee, shares, vault.total_tokens, vault.total_shares
        );
        Ok(())
    }
}

/// 1% от суммы, округление вниз.
fn deposit_fee(amount: u64) -> Result<u64> {
    let fee = (amount as u128)
        .checked_mul(DEPOSIT_FEE_BPS as u128)
        .ok_or(VaultError::MathOverflow)?
        / BPS_DENOMINATOR as u128;
    Ok(fee as u64)
}

fn shares_for_deposit(net: u64, total_tokens: u64, total_shares: u64) -> Result<u64> {
    if total_shares == 0 || total_tokens == 0 {
        return Ok(net);
    }
    (net as u128)
        .checked_mul(total_shares as u128)
        .and_then(|v| v.checked_div(total_tokens as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))
}

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(
        init,
        payer = admin,
        space = VaultState::LEN,
        seeds = [b"vault", token_mint.key().as_ref()],
        bump,
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        init,
        payer = admin,
        token::mint = token_mint,
        token::authority = vault_state,
        seeds = [b"vault_tokens", token_mint.key().as_ref()],
        bump,
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        mint::decimals = 6,
        mint::authority = vault_state,
        seeds = [b"share_mint", token_mint.key().as_ref()],
        bump,
    )]
    pub share_mint: Account<'info, Mint>,

    pub token_mint: Account<'info, Mint>,

    /// Токен-аккаунт (того же mint), куда уходит комиссия 1% с депозитов
    #[account(token::mint = token_mint)]
    pub fee_wallet: Account<'info, TokenAccount>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CreateUserCard<'info> {
    #[account(
        seeds = [b"vault", vault_state.token_mint.as_ref()],
        bump = vault_state.bump,
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        init,
        payer = user,
        space = UserCard::LEN,
        seeds = [b"user_card", vault_state.key().as_ref(), user.key().as_ref()],
        bump,
    )]
    pub user_card: Account<'info, UserCard>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault_state.token_mint.as_ref()],
        bump = vault_state.bump,
    )]
    pub vault_state: Account<'info, VaultState>,

    /// Депозит возможен только после create_user_card
    #[account(
        mut,
        seeds = [b"user_card", vault_state.key().as_ref(), user.key().as_ref()],
        bump = user_card.bump,
    )]
    pub user_card: Account<'info, UserCard>,

    #[account(
        mut,
        seeds = [b"vault_tokens", vault_state.token_mint.as_ref()],
        bump,
        token::mint = vault_state.token_mint,
        token::authority = vault_state,
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"share_mint", vault_state.token_mint.as_ref()],
        bump,
        mint::authority = vault_state,
    )]
    pub share_mint: Account<'info, Mint>,

    /// Только тот fee_wallet, что сохранён в vault при создании
    #[account(mut, address = vault_state.fee_wallet @ VaultError::InvalidFeeWallet)]
    pub fee_wallet: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = vault_state.token_mint,
        token::authority = user,
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = share_mint,
        token::authority = user,
    )]
    pub user_share_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

#[account]
#[derive(Default)]
pub struct VaultState {
    pub admin: Pubkey,
    pub token_mint: Pubkey,
    pub vault_token_account: Pubkey,
    pub share_mint: Pubkey,
    pub fee_wallet: Pubkey,
    pub total_tokens: u64,
    pub total_shares: u64,
    pub min_deposit: u64,
    pub bump: u8,
    pub paused: bool,
}

impl VaultState {
    pub const LEN: usize = 8 + 32 + 32 + 32 + 32 + 32 + 8 + 8 + 8 + 1 + 1;
}

#[account]
#[derive(Default)]
pub struct UserCard {
    pub vault: Pubkey,
    pub owner: Pubkey,
    pub bump: u8,
    pub created_at: i64,
    /// Сколько токенов зачислено в vault (после комиссии)
    pub deposited: u64,
    pub shares: u64,
}

impl UserCard {
    pub const LEN: usize = 8 + 32 + 32 + 1 + 8 + 8 + 8;
}

#[derive(Accounts)]
pub struct SetPaused<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault_state.token_mint.as_ref()],
        bump = vault_state.bump,
        has_one = admin
    )]
    pub vault_state: Account<'info, VaultState>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct CloseUserCard<'info> {
    #[account(
        seeds = [b"vault", vault_state.token_mint.as_ref()],
        bump = vault_state.bump
    )]
    pub vault_state: Account<'info, VaultState>,

    /// Карточку с shares закрыть нельзя — иначе потеряется учёт депозита
    #[account(
        mut,
        close = user,
        seeds = [b"user_card", vault_state.key().as_ref(), user.key().as_ref()],
        bump = user_card.bump,
        constraint = user_card.shares == 0 @ VaultError::CardHasShares
    )]
    pub user_card: Account<'info, UserCard>,

    #[account(mut)]
    pub user: Signer<'info>,
}

#[error_code]
pub enum VaultError {
    #[msg("Vault is paused")]
    VaultPaused,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Deposit is below the minimum (1 token)")]
    DepositTooSmall,
    #[msg("Shares would be zero - deposit too small")]
    SharesTooSmall,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Fee wallet does not match the vault")]
    InvalidFeeWallet,
    #[msg("Card still has shares")]
    CardHasShares,
}
