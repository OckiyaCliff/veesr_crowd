use anchor_lang::prelude::*;
use anchor_lang::system_program;

// This is the program's on-chain address.
// When you build with `anchor build`, it will be updated.
// For Solana Playground, you can leave it as the default or update it after deploying.
declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

// The wallet that will receive the platform fees.
// REPLACE THIS with your actual platform fee wallet address.
const PLATFORM_WALLET: &str = "Gf2t3iS1MTkLpn3d2hWqrM3p4Wzt5iWj2iFv2a4v5z7b"; // Example Address
const PLATFORM_FEE_BPS: u64 = 300; // 300 basis points = 3%
const BPS_DIVISOR: u64 = 10000;

#[program]
pub mod veesr_programs {
    use super::*;

    /// Creates a new campaign account and initializes it with the given parameters.
    pub fn create_campaign(
        ctx: Context<CreateCampaign>,
        title: String,
        description: String,
        target_amount: u64,
        location: String,
        metrics: Vec<String>,
        media_uris: Vec<String>,
        category: CampaignCategory,
    ) -> Result<()> {
        let campaign = &mut ctx.accounts.campaign;
        let clock = Clock::get()?;

        // Add validation checks
        require!(target_amount > 0, VeesrError::InvalidTargetAmount);
        require!(!title.is_empty() && title.len() <= 50, VeesrError::InvalidTitle);
        require!(!description.is_empty() && description.len() <= 500, VeesrError::InvalidDescription);

        // Set the campaign properties
        campaign.authority = ctx.accounts.authority.key();
        campaign.title = title;
        campaign.description = description;
        campaign.target_amount = target_amount;
        campaign.current_amount = 0; // Starts with 0 funds
        campaign.location = location;
        campaign.metrics = metrics;
        campaign.media_uris = media_uris;
        campaign.created_at = clock.unix_timestamp;
        campaign.deadline = clock.unix_timestamp + (30 * 24 * 60 * 60); // Default 30-day deadline
        campaign.status = CampaignStatus::Active;
        campaign.category = category;

        msg!("Campaign '{}' created successfully!", campaign.title);
        Ok(())
    }

    /// Allows a user to donate to an active campaign.
    /// This function now also creates a `DonationReceipt` account to track the donation.
    pub fn donate_to_campaign(ctx: Context<DonateToCampaign>, amount: u64) -> Result<()> {
        let campaign = &mut ctx.accounts.campaign;
        let clock = Clock::get()?;

        // Validation checks
        require!(amount > 0, VeesrError::InvalidDonationAmount);
        require!(campaign.status == CampaignStatus::Active, VeesrError::CampaignNotActive);
        require!(clock.unix_timestamp < campaign.deadline, VeesrError::CampaignExpired);

        // Create the on-chain donation receipt
        let receipt = &mut ctx.accounts.donation_receipt;
        receipt.donor = ctx.accounts.donor.key();
        receipt.campaign = campaign.key();
        receipt.amount = amount;
        receipt.timestamp = clock.unix_timestamp;

        // Perform the SOL transfer from donor to the campaign PDA
        let cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.donor.to_account_info(),
                to: campaign.to_account_info(),
            },
        );
        system_program::transfer(cpi_context, amount)?;

        // Update the campaign's current amount
        campaign.current_amount = campaign.current_amount.checked_add(amount).unwrap();

        msg!("Donation of {} lamports received. Receipt created.", amount);

        // Check if the campaign has reached its funding goal
        if campaign.current_amount >= campaign.target_amount {
            campaign.status = CampaignStatus::Funded;
            msg!("Campaign '{}' is now fully funded!", campaign.title);
        }

        Ok(())
    }

    /// Allows the campaign authority to withdraw funds and complete the campaign.
    /// This now includes logic to send a 3% platform fee.
    pub fn withdraw_and_complete(ctx: Context<WithdrawAndComplete>) -> Result<()> {
        let campaign = &ctx.accounts.campaign;

        // Validation checks
        require!(campaign.status == CampaignStatus::Funded, VeesrError::CampaignNotFunded);

        // Calculate the fee and the amount for the executor
        let total_amount = campaign.current_amount;
        let fee = total_amount.checked_mul(PLATFORM_FEE_BPS).unwrap().checked_div(BPS_DIVISOR).unwrap();
        let amount_to_executor = total_amount.checked_sub(fee).unwrap();

        // Get the PDA signer seeds
        let authority_key = campaign.authority.key();
        let seeds = &[&b"campaign"[..], authority_key.as_ref(), &[ctx.bumps.campaign]];
        let signer_seeds = &[&seeds[..]];

        // 1. Transfer the platform fee
        if fee > 0 {
            let cpi_accounts_fee = system_program::Transfer {
                from: campaign.to_account_info(),
                to: ctx.accounts.platform_wallet.to_account_info(),
            };
            let cpi_program_fee = ctx.accounts.system_program.to_account_info();
            let cpi_context_fee = CpiContext::new_with_signer(cpi_program_fee, cpi_accounts_fee, signer_seeds);
            system_program::transfer(cpi_context_fee, fee)?;
        }
        
        // 2. Transfer the remaining funds to the executor
        if amount_to_executor > 0 {
            let cpi_accounts_executor = system_program::Transfer {
                from: campaign.to_account_info(),
                to: ctx.accounts.executor.to_account_info(),
            };
            let cpi_program_executor = ctx.accounts.system_program.to_account_info();
            let cpi_context_executor = CpiContext::new_with_signer(cpi_program_executor, cpi_accounts_executor, signer_seeds);
            system_program::transfer(cpi_context_executor, amount_to_executor)?;
        }

        msg!(
            "Withdrawal complete. Executor received: {}. Platform fee: {}.",
            amount_to_executor,
            fee
        );

        Ok(())
    }

    /// Allows the campaign authority to cancel an active campaign.
    /// If the campaign has no donations, it is closed immediately.
    /// If it has donations, its status is simply updated to `Cancelled`,
    /// allowing donors to claim refunds separately.
    pub fn cancel_campaign(ctx: Context<CancelCampaign>) -> Result<()> {
        let campaign = &mut ctx.accounts.campaign;
        let clock = Clock::get()?;

        // A campaign can be cancelled if it's still active,
        // OR if it has passed its deadline without being successfully funded.
        let is_expired = clock.unix_timestamp > campaign.deadline;
        let can_be_cancelled = campaign.status == CampaignStatus::Active || (is_expired && campaign.status != CampaignStatus::Funded);
        
        require!(can_be_cancelled, VeesrError::CannotCancelCampaign);

        // If there are no donations, we can close the account directly.
        // The `close` constraint on the context will handle the rent refund.
        if campaign.current_amount == 0 {
            msg!("Campaign '{}' has no donations and is being closed.", campaign.title);
            // The account closing is handled by the `close` constraint automatically.
            // No status change needed as the account will cease to exist.
            return Ok(());
        }

        // If there are donations, we mark the campaign as cancelled so donors can claim refunds.
        campaign.status = CampaignStatus::Cancelled;
        
        msg!(
            "Campaign '{}' has been cancelled. Donors can now claim refunds.",
            campaign.title
        );

        Ok(())
    }

    /// Allows a donor to claim a refund from a cancelled campaign.
    /// This function verifies the original donation via the `DonationReceipt` account,
    /// transfers the funds back to the donor, and closes the receipt account.
    pub fn claim_refund(ctx: Context<ClaimRefund>) -> Result<()> {
        let campaign = &mut ctx.accounts.campaign;
        let receipt = &ctx.accounts.donation_receipt;

        // Security checks
        require!(campaign.status == CampaignStatus::Cancelled, VeesrError::CampaignNotCancelled);
        require!(receipt.donor == ctx.accounts.donor.key(), VeesrError::InvalidRefundRequest);

        // Transfer funds from the campaign PDA back to the donor.
        let amount_to_refund = receipt.amount;
        
        let authority_key = campaign.authority;
        let campaign_seeds = &[&b"campaign"[..], authority_key.as_ref(), &[ctx.bumps.campaign]];
        let signer_seeds = &[&campaign_seeds[..]];
        
        let cpi_accounts = system_program::Transfer {
            from: campaign.to_account_info(),
            to: ctx.accounts.donor.to_account_info(),
        };
        let cpi_program = ctx.accounts.system_program.to_account_info();
        let cpi_context = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);

        system_program::transfer(cpi_context, amount_to_refund)?;

        // Decrement the campaign's total amount
        campaign.current_amount = campaign.current_amount.checked_sub(amount_to_refund).unwrap();

        msg!("Refund of {} lamports successful.", amount_to_refund);
        
        // The DonationReceipt account is closed automatically by the `close` constraint on the context.
        Ok(())
    }
}

/// The context for the `create_campaign` instruction.
/// It defines all the accounts that are required.
#[derive(Accounts)]
pub struct CreateCampaign<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Campaign::INIT_SPACE, // 8 bytes for the anchor discriminator + space for the Campaign struct
        seeds = [b"campaign", authority.key().as_ref()], // Ensures each user can have one campaign PDA for this seed
        bump
    )]
    pub campaign: Account<'info, Campaign>,

    #[account(mut)]
    pub authority: Signer<'info>,
    
    pub system_program: Program<'info, System>,
}

/// The context for the `donate_to_campaign` instruction.
#[derive(Accounts)]
pub struct DonateToCampaign<'info> {
    #[account(mut)]
    pub campaign: Account<'info, Campaign>,

    #[account(mut)]
    pub donor: Signer<'info>,

    #[account(
        init,
        payer = donor,
        space = 8 + DonationReceipt::INIT_SPACE,
        // Seeds ensure one donation receipt per donor per campaign.
        // This prevents a donor from creating multiple receipts for the same campaign.
        seeds = [b"donation", campaign.key().as_ref(), donor.key().as_ref()],
        bump
    )]
    pub donation_receipt: Account<'info, DonationReceipt>,

    pub system_program: Program<'info, System>,
}

/// The context for the `withdraw_and_complete` instruction.
#[derive(Accounts)]
pub struct WithdrawAndComplete<'info> {
    #[account(
        mut,
        // The `close` constraint marks the account for closure and refunds the rent lamports
        // to the specified address, in this case, the original campaign authority.
        close = authority,
        // `has_one` is a security check that ensures the `authority` signer account
        // matches the `authority` key stored in the `campaign` account.
        has_one = authority,
        seeds = [b"campaign", authority.key().as_ref()],
        bump
    )]
    pub campaign: Account<'info, Campaign>,

    #[account(mut)]
    pub authority: Signer<'info>,

    /// The wallet account that will receive the funds from the campaign.
    /// Marked as a SystemAccount to ensure it's a standard user wallet.
    #[account(mut)]
    pub executor: SystemAccount<'info>,

    /// The wallet that will receive the platform fee.
    #[account(
        mut,
        address = PLATFORM_WALLET.parse::<Pubkey>().unwrap() @ VeesrError::InvalidPlatformWallet
    )]
    pub platform_wallet: SystemAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// The context for the `cancel_campaign` instruction.
#[derive(Accounts)]
pub struct CancelCampaign<'info> {
    #[account(
        mut,
        // If current_amount is 0, close the account and refund rent to the authority.
        // Otherwise, the account remains open for refunds.
        close = authority,
        has_one = authority
    )]
    pub campaign: Account<'info, Campaign>,

    #[account(mut)]
    pub authority: Signer<'info>,
}

/// The context for the `claim_refund` instruction.
#[derive(Accounts)]
pub struct ClaimRefund<'info> {
    #[account(
        mut,
        // Re-seed the campaign PDA to verify it and access the bump
        seeds = [b"campaign", campaign.authority.as_ref()],
        bump
    )]
    pub campaign: Account<'info, Campaign>,

    #[account(mut)]
    pub donor: Signer<'info>,

    #[account(
        mut,
        // Close the receipt account after the refund to recover its rent.
        close = donor,
        // Security checks to ensure the receipt matches the campaign and the donor.
        has_one = campaign,
        has_one = donor,
        seeds = [b"donation", campaign.key().as_ref(), donor.key().as_ref()],
        bump
    )]
    pub donation_receipt: Account<'info, DonationReceipt>,

    pub system_program: Program<'info, System>,
}

/// Stores a record of a single donation.
#[account]
#[derive(InitSpace)]
pub struct DonationReceipt {
    pub donor: Pubkey,
    pub campaign: Pubkey,
    pub amount: u64,
    pub timestamp: i64,
}

/// The main account that holds all the data for a campaign.
/// `#[derive(InitSpace)]` automatically calculates the required space,
/// but we must provide `#[max_len]` for dynamic types like String and Vec.
#[account]
#[derive(InitSpace)]
pub struct Campaign {
    pub authority: Pubkey,
    pub target_amount: u64,
    pub current_amount: u64,
    pub deadline: i64,
    pub created_at: i64,
    pub status: CampaignStatus,
    pub category: CampaignCategory,
    #[max_len(50)]      // Max length for the campaign title
    pub title: String,
    #[max_len(500)]     // Max length for the campaign description
    pub description: String,
    #[max_len(100)]     // Max length for the location string
    pub location: String,
    #[max_len(5, 50)]   // Max 5 metrics, each with a max length of 50 characters
    pub metrics: Vec<String>,
    #[max_len(5, 100)]  // Max 5 media URIs, each with a max length of 100 characters
    pub media_uris: Vec<String>,
}

/// Defines the possible statuses a campaign can be in.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, InitSpace)]
pub enum CampaignStatus {
    Active,
    Funded,
    InProgress,
    Completed,
    Expired,
    Cancelled,
}

/// Defines the categories for campaigns to allow for filtering.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, InitSpace)]
pub enum CampaignCategory {
    Health,
    Water,
    Education,
    Energy,
    Infrastructure,
    Emergency,
    Other,
}

#[error_code]
pub enum VeesrError {
    #[msg("Title is empty or too long.")]
    InvalidTitle,
    #[msg("Description is empty or too long.")]
    InvalidDescription,
    #[msg("Target amount must be greater than zero.")]
    InvalidTargetAmount,
    #[msg("Donation amount must be greater than zero.")]
    InvalidDonationAmount,
    #[msg("The campaign is not active.")]
    CampaignNotActive,
    #[msg("The campaign has already expired.")]
    CampaignExpired,
    #[msg("The campaign has not been fully funded yet.")]
    CampaignNotFunded,
    #[msg("This campaign cannot be cancelled at its current state.")]
    CannotCancelCampaign,
    #[msg("This campaign has not been cancelled.")]
    CampaignNotCancelled,
    #[msg("The signer of this transaction is not the original donor.")]
    InvalidRefundRequest,
    #[msg("The provided platform wallet is incorrect.")]
    InvalidPlatformWallet,
}
