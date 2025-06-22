// This script can be used in Solana Playground's client.ts editor.
// 1. Build and deploy your program.
// 2. Paste this code into the editor.
// 3. Click "Run".

// Note: You may need to run this script multiple times if you encounter
// transaction timeouts on devnet.

(async () => {
  // Constants
  const platformWallet = pg.wallet.keypair; // Using the playground wallet as the platform fee collector
  const campaignAuthority = pg.wallet.keypair;
  const executor = anchor.web3.Keypair.generate();
  const donor1 = anchor.web3.Keypair.generate();
  const donor2 = anchor.web3.Keypair.generate();

  const program = pg.program;
  const connection = pg.connection;

  // Helper to request airdrop and log
  async function airdrop(wallet: anchor.web3.Keypair, amount: number) {
    console.log(`Airdropping ${amount} SOL to ${wallet.publicKey.toBase58()}...`);
    const sig = await connection.requestAirdrop(wallet.publicKey, amount * anchor.web3.LAMPORTS_PER_SOL);
    await connection.confirmTransaction(sig, "confirmed");
    console.log("Airdrop complete.");
  }

  // Airdrop to all our test wallets
  await airdrop(executor, 1);
  await airdrop(donor1, 5);
  await airdrop(donor2, 5);

  console.log("------ Starting Test: Happy Path (Create -> Donate -> Withdraw) ------");

  // --- 1. Create a campaign ---
  const campaignId1 = "campaign_" + Math.random().toString(36).substring(2);
  const targetAmount1 = new anchor.BN(2 * anchor.web3.LAMPORTS_PER_SOL); // 2 SOL

  const [campaignPDA1] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("campaign"), Buffer.from(campaignId1)],
    program.programId
  );

  console.log(`Creating campaign 1 with ID: ${campaignId1}`);
  try {
    await program.methods
      .createCampaign(
        campaignId1,
        "Health Drive",
        "Raising funds for local clinic.",
        targetAmount1,
        { health: {} }
      )
      .accounts({
        campaign: campaignPDA1,
        authority: campaignAuthority.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([campaignAuthority])
      .rpc({ commitment: "confirmed" });

    let campaignAccount1 = await program.account.campaign.fetch(campaignPDA1);
    console.log("Campaign 1 created successfully!");
    console.log(` -> Raised: ${campaignAccount1.raisedAmount}, Target: ${campaignAccount1.targetAmount}`);
    console.log(` -> Status: ${Object.keys(campaignAccount1.status)[0]}`);
  } catch (e) {
    console.error("Error creating campaign 1:", e);
    return; // Exit if creation fails
  }


  // --- 2. Donate to the campaign ---
  const donationAmount1 = new anchor.BN(1 * anchor.web3.LAMPORTS_PER_SOL); // 1 SOL

  // Donor 1 donates
  const [donationReceiptPDA1] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("donation"), campaignPDA1.toBuffer(), donor1.publicKey.toBuffer()],
    program.programId
  );

  console.log(`Donor 1 (${donor1.publicKey.toBase58()}) donating ${donationAmount1.toString()} lamports...`);
  try {
     await program.methods
      .donateToCampaign(donationAmount1)
      .accounts({
        campaign: campaignPDA1,
        donationReceipt: donationReceiptPDA1,
        donor: donor1.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([donor1])
      .rpc({ commitment: "confirmed" });

    let campaignAccount1 = await program.account.campaign.fetch(campaignPDA1);
    console.log("Donor 1 donation successful!");
    console.log(` -> Raised: ${campaignAccount1.raisedAmount}, Target: ${campaignAccount1.targetAmount}`);
    console.log(` -> Status: ${Object.keys(campaignAccount1.status)[0]}`);
  } catch(e) {
    console.error("Error with Donor 1's donation:", e);
    return;
  }


  // Donor 2 donates, meeting the goal
   const [donationReceiptPDA2] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("donation"), campaignPDA1.toBuffer(), donor2.publicKey.toBuffer()],
    program.programId
  );
  console.log(`Donor 2 (${donor2.publicKey.toBase58()}) donating ${donationAmount1.toString()} lamports...`);
  try {
     await program.methods
      .donateToCampaign(donationAmount1)
      .accounts({
        campaign: campaignPDA1,
        donationReceipt: donationReceiptPDA2,
        donor: donor2.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([donor2])
      .rpc({ commitment: "confirmed" });

    let campaignAccount1 = await program.account.campaign.fetch(campaignPDA1);
    console.log("Donor 2 donation successful! Campaign should now be funded.");
    console.log(` -> Raised: ${campaignAccount1.raisedAmount}, Target: ${campaignAccount1.targetAmount}`);
    console.log(` -> Status: ${Object.keys(campaignAccount1.status)[0]}`);
    if (Object.keys(campaignAccount1.status)[0] !== 'funded') {
        console.error("TEST FAILED: Campaign status did not update to Funded.");
    }
  } catch(e) {
    console.error("Error with Donor 2's donation:", e);
    return;
  }

  // --- 3. Withdraw funds ---
  console.log("Authority withdrawing funds to executor...");
  try {
    const executorBalanceBefore = await connection.getBalance(executor.publicKey);
    const platformBalanceBefore = await connection.getBalance(platformWallet.publicKey);

    await program.methods
      .withdrawAndComplete()
      .accounts({
        campaign: campaignPDA1,
        authority: campaignAuthority.publicKey,
        executor: executor.publicKey,
        platformWallet: platformWallet.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([campaignAuthority])
      .rpc({ commitment: "confirmed" });
    
    console.log("Withdrawal successful!");
    const executorBalanceAfter = await connection.getBalance(executor.publicKey);
    const platformBalanceAfter = await connection.getBalance(platformWallet.publicKey);

    const expectedFee = targetAmount1.toNumber() * 0.03;
    const expectedExecutorAmount = targetAmount1.toNumber() - expectedFee;

    console.log(` -> Executor Balance Change: ${executorBalanceAfter - executorBalanceBefore} (Expected: ~${expectedExecutorAmount})`);
    console.log(` -> Platform Balance Change: ${platformBalanceAfter - platformBalanceBefore} (Expected: ~${expectedFee})`);
    
    // Check if campaign account is closed
    const closedAccountInfo = await connection.getAccountInfo(campaignPDA1);
    if (closedAccountInfo === null) {
        console.log(" -> Campaign account successfully closed.");
    } else {
        console.error("TEST FAILED: Campaign account was not closed after withdrawal.");
    }

  } catch (e) {
    console.error("Error during withdrawal:", e);
  }

  console.log("\n------ Test Happy Path Complete ------\n");


  console.log("------ Starting Test: Refund Path (Create -> Donate -> Cancel -> Refund) ------");

  // --- 1. Create a second campaign ---
  const campaignId2 = "campaign_" + Math.random().toString(36).substring(2);
  const targetAmount2 = new anchor.BN(3 * anchor.web3.LAMPORTS_PER_SOL); // 3 SOL

  const [campaignPDA2] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("campaign"), Buffer.from(campaignId2)],
    program.programId
  );

  console.log(`Creating campaign 2 with ID: ${campaignId2}`);
  await program.methods
      .createCampaign(
        campaignId2,
        "Art Project",
        "Community mural.",
        targetAmount2,
        { art: {} }
      )
      .accounts({
        campaign: campaignPDA2,
        authority: campaignAuthority.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([campaignAuthority])
      .rpc({ commitment: "confirmed" });
  console.log("Campaign 2 created.");

  // --- 2. Donor 1 donates to second campaign ---
   const [donationReceiptPDA3] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("donation"), campaignPDA2.toBuffer(), donor1.publicKey.toBuffer()],
    program.programId
  );
  console.log(`Donor 1 donating ${donationAmount1.toString()} lamports to campaign 2...`);
  await program.methods
      .donateToCampaign(donationAmount1)
      .accounts({
        campaign: campaignPDA2,
        donationReceipt: donationReceiptPDA3,
        donor: donor1.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([donor1])
      .rpc({ commitment: "confirmed" });
  console.log("Donation successful.");
  
  // --- 3. Cancel the campaign ---
  console.log("Authority canceling campaign 2...");
  try {
    await program.methods
      .cancelCampaign()
      .accounts({
        campaign: campaignPDA2,
        authority: campaignAuthority.publicKey,
      })
      .signers([campaignAuthority])
      .rpc({ commitment: "confirmed" });
      
    let campaignAccount2 = await program.account.campaign.fetch(campaignPDA2);
    console.log("Campaign 2 canceled successfully.");
    console.log(` -> Status: ${Object.keys(campaignAccount2.status)[0]}`);
    if (Object.keys(campaignAccount2.status)[0] !== 'cancelled') {
        console.error("TEST FAILED: Campaign status did not update to Cancelled.");
    }
  } catch (e) {
    console.error("Error canceling campaign:", e);
  }

  // --- 4. Donor claims refund ---
  console.log("Donor 1 claiming refund...");
  try {
    const donor1BalanceBefore = await connection.getBalance(donor1.publicKey);
    
    // We need to pass the donation receipt PDA to the instruction.
    // It was already derived above as donationReceiptPDA3
    await program.methods
      .claimRefund()
      .accounts({
        campaign: campaignPDA2,
        donationReceipt: donationReceiptPDA3,
        donor: donor1.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([donor1])
      .rpc({ commitment: "confirmed" });

    console.log("Refund claimed successfully!");
    const donor1BalanceAfter = await connection.getBalance(donor1.publicKey);

    // Note: Balance change won't be exactly the donation amount due to gas fees on the refund tx.
    console.log(` -> Donor 1 Balance Before: ${donor1BalanceBefore}`);
    console.log(` -> Donor 1 Balance After:  ${donor1BalanceAfter}`);
    console.log(` -> Balance Change: ${donor1BalanceAfter - donor1BalanceBefore} (approx ${donationAmount1.toString()})`);
    
    const receiptAccountInfo = await connection.getAccountInfo(donationReceiptPDA3);
    if (receiptAccountInfo === null) {
        console.log(" -> Donation receipt account successfully closed.");
    } else {
        console.error("TEST FAILED: Donation receipt was not closed after refund.");
    }

  } catch(e) {
    console.error("Error claiming refund:", e);
  }

  console.log("\n------ Test Refund Path Complete ------");

})().catch(err => {
  console.error(err);
});

