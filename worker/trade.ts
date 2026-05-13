import { TonClient, WalletContractV4, internal, toNano, Address } from "@ton/ton";
import { mnemonicToPrivateKey } from "@ton/crypto";
import { DEX, pTON } from "@ston-fi/sdk";
import * as dotenv from "dotenv";

dotenv.config({ path: "../.env" });

async function main() {
    console.log("TS Worker starting...");
    const args = process.argv.slice(2);
    if (args.length < 3) {
        console.error("Usage: ts-node trade.ts <buy|sell> <amount> <tokenAddress>");
        process.exit(1);
    }

    const action = args[0].toLowerCase();
    const amountStr = args[1];
    const jettonAddressStr = args[2];

    const isTestnet = process.env.TESTNET === "true";
    console.log(`Mode: ${isTestnet ? "Testnet" : "Mainnet"}`);
    
    const endpoint = isTestnet ? "https://testnet.toncenter.com/api/v2/jsonRPC" : "https://toncenter.com/api/v2/jsonRPC";

    const client = new TonClient({ endpoint });

    const seed = process.env.TON_SEED || "";
    if (!seed) throw new Error("TON_SEED not set");
    
    const keyPair = await mnemonicToPrivateKey(seed.split(" "));
    const wallet = WalletContractV4.create({ workchain: 0, publicKey: keyPair.publicKey });
    const contract = client.open(wallet);

    // Testnet addresses from STON.fi docs for v2.1.0
    const ROUTER_ADDRESS = isTestnet ? "kQALh-JBBIKK7gr0o4AVf9JZnEsFndqO0qTCyT-D-yBsWk0v" : "EQBfBWT7X2BLeHlklxS4x8RGNVX8mP5X9y4o0B2m30E-_k5P";
    const PTON_ADDRESS = isTestnet ? "kQACS30DNoUQ7NfApPvzh7eBmSZ9L4ygJ-lkNWtba8TQT-Px" : "EQCM3B12QK1e4yZSf8GtBRT0aLMNyEsBc_DhVfRRtOEffLez";

    const router = client.open(DEX.v2_1.Router.create(Address.parse(ROUTER_ADDRESS)));
    const proxyTon = pTON.v2_1.create(Address.parse(PTON_ADDRESS));
    const jettonAddress = Address.parse(jettonAddressStr);


    if (action === "buy") {
        console.log(`Executing BUY on ${isTestnet ? "Testnet" : "Mainnet"}: Swapping ${amountStr} TON for Jetton ${jettonAddress.toString()}`);
        
        let success = false;
        for (let attempt = 1; attempt <= 7; attempt++) {
            try {
                console.log(`Attempt ${attempt}/7 to get swap parameters...`);
                const txParams = await router.getSwapTonToJettonTxParams({
                    userWalletAddress: wallet.address,
                    proxyTon: proxyTon,
                    offerAmount: toNano(amountStr),
                    askJettonAddress: jettonAddress,
                    minAskAmount: 1n,
                    queryId: 12345n,
                });

                console.log("Fetching seqno...");
                const seqno = await contract.getSeqno();
                
                const transfer = contract.createTransfer({
                    seqno,
                    secretKey: keyPair.secretKey,
                    messages: [
                        internal({
                            to: txParams.to,
                            value: txParams.value,
                            body: txParams.body,
                        })
                    ]
                });

                const hash = transfer.hash().toString("hex");
                console.log(`TX_HASH: ${hash}`);
                
                await contract.send(transfer);
                console.log("TX_SENT_SUCCESSFULLY");
                success = true;
                break;
            } catch (err: any) {
                console.error(`Attempt ${attempt} failed: ${err.message}`);
                if (attempt < 7) {
                    console.log("Waiting 15 seconds for next retry...");
                    await new Promise(resolve => setTimeout(resolve, 15000));
                }
            }
        }

        if (!success) {
            throw new Error("Failed to execute buy after 7 attempts. Most likely no liquidity pool yet.");
        }


    } else if (action === "sell") {
        console.log(`Executing SELL on ${isTestnet ? "Testnet" : "Mainnet"}: Swapping ${amountStr} Jetton for TON`);
        
        const txParams = await router.getSwapJettonToTonTxParams({
            userWalletAddress: wallet.address,
            offerJettonAddress: jettonAddress,
            offerAmount: toNano(amountStr),
            minAskAmount: 1n,
            proxyTon: proxyTon,
            queryId: 12345n,
        });

        console.log("Fetching seqno...");
        const seqno = await contract.getSeqno();

        const transfer = contract.createTransfer({
            seqno,
            secretKey: keyPair.secretKey,
            messages: [
                internal({
                    to: txParams.to,
                    value: txParams.value,
                    body: txParams.body,
                })
            ]
        });

        const hash = transfer.hash().toString("hex");
        console.log(`TX_HASH: ${hash}`);

        await contract.send(transfer);
        console.log("TX_SENT_SUCCESSFULLY");
    }
 else {
        console.error("Invalid action");
    }
}

main().catch(err => {
    console.error("TS Worker Error:", err);
    process.exit(1);
});
