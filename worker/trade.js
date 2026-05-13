import { TonClient, WalletContractV4, internal, toNano } from "@ton/ton";
import { mnemonicToPrivateKey } from "@ton/crypto";
import { DEX, pTON } from "@ston-fi/sdk";
import * as dotenv from "dotenv";
dotenv.config({ path: "../.env" });
async function main() {
    const args = process.argv.slice(2);
    if (args.length < 3) {
        console.error("Usage: ts-node trade.ts <buy|sell> <amount> <tokenAddress>");
        process.exit(1);
    }
    const action = args[0].toLowerCase();
    const amountStr = args[1];
    const jettonAddress = args[2];
    const isTestnet = process.env.TESTNET === "true";
    const endpoint = isTestnet ? "https://testnet.toncenter.com/api/v2/jsonRPC" : "https://toncenter.com/api/v2/jsonRPC";
    const client = new TonClient({ endpoint });
    const seed = process.env.TON_SEED || "";
    if (!seed)
        throw new Error("TON_SEED not set");
    const keyPair = await mnemonicToPrivateKey(seed.split(" "));
    const wallet = WalletContractV4.create({ workchain: 0, publicKey: keyPair.publicKey });
    const contract = client.open(wallet);
    // Testnet addresses from STON.fi docs for v2.1.0
    const ROUTER_ADDRESS = isTestnet ? "kQALh-JBBIKK7gr0o4AVf9JZnEsFndqO0qTCyT-D-yBsWk0v" : "EQBfBWT7X2BLeHlklxS4x8RGNVX8mP5X9y4o0B2m30E-_k5P";
    const PTON_ADDRESS = isTestnet ? "kQACS30DNoUQ7NfApPvzh7eBmSZ9L4ygJ-lkNWtba8TQT-Px" : "EQCM3B12QK1e4yZSf8GtBRT0aLMNyEsBc_DhVfRRtOEffLez";
    const router = client.open(DEX.v2_1.Router.create(ROUTER_ADDRESS));
    const proxyTon = pTON.v2_1.create(PTON_ADDRESS);
    if (action === "buy") {
        console.log(`Executing BUY on ${isTestnet ? "Testnet" : "Mainnet"}: Swapping ${amountStr} TON for Jetton ${jettonAddress}`);
        const txParams = await router.getSwapTonToJettonTxParams({
            userWalletAddress: wallet.address,
            proxyTon: proxyTon,
            offerAmount: toNano(amountStr),
            askJettonAddress: jettonAddress,
            minAskAmount: 1n, // Minimal slippage protection for testing
            queryId: 12345n,
        });
        const seqno = await contract.getSeqno();
        await contract.sendTransfer({
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
        console.log("BUY transaction sent successfully!");
    }
    else if (action === "sell") {
        console.log(`Executing SELL on ${isTestnet ? "Testnet" : "Mainnet"}: Swapping ${amountStr} Jetton for TON`);
        const txParams = await router.getSwapJettonToTonTxParams({
            userWalletAddress: wallet.address,
            offerJettonAddress: jettonAddress,
            offerAmount: toNano(amountStr), // Assumes 9 decimals, need to fetch real decimals ideally
            minAskAmount: 1n,
            proxyTon: proxyTon,
            queryId: 12345n,
        });
        const seqno = await contract.getSeqno();
        await contract.sendTransfer({
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
        console.log("SELL transaction sent successfully!");
    }
    else {
        console.error("Invalid action");
    }
}
main().catch(console.error);
