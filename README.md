# octl

# 📝 Overview

**$OTCL** is a high-frequency trading (HFT) OTC limit order execution protocol on Solana. It enables institutional and large-scale traders to place limit orders off-chain or on-chain while ensuring price stability and minimal market impact.

**NOTE CURRENT VERSION IS ONLY DEVELOPED IN SOLANA PLAYGROUND AND NEXT VERSION WILL BE EXPORTED TO VSCODE**

## 🔹 How It Works:

- Traders lock collateral in **$OTCL tokens** to create large-limit OTC orders.
- Liquidity providers (market makers) fulfill orders and earn **$OTCL token rewards**.
- **Priority staking** boosts order execution speed & reduces fees.
- **Multisig approval** allows institutional & DAO traders to authorize orders.
- **Commit-Reveal anti-frontrunning** prevents MEV exploits.
- **Fee rebates & governance vault** accumulate treasury funds.

---

## ✨ Features

✅ **Limit Order Execution** – Traders create OTC limit orders with a price, quantity, and expiration time.  
✅ **Automatic Order Matching** – Orders are filled by liquidity providers at the best available price.  
✅ **Maker-Taker Fee Model** – Market makers earn rebates, and order takers pay small fees (discounts for stakers).  
✅ **Multi-Signature Support** – Institutional & DAO traders can require multiple approvals before execution.  
✅ **Anti-Frontrunning (Commit-Reveal)** – Traders can commit orders off-chain, preventing MEV exploits.  
✅ **VIP Staking Tiers** – High-stake traders get faster execution & lower fees.  
✅ **On-Chain Reputation System** – Traders & market makers build trust scores based on volume & execution speed.  
✅ **Treasury & Governance** – A portion of trading fees is stored in a treasury vault, governed by a DAO/multisig.  

---

## 💾 Program Structure

The smart contract consists of the following on-chain instructions:

### 1️⃣ Order Management

| Instruction | Description |
|------------|------------|
| `create_order(price, quantity, ttl, is_multisig, multisig_threshold)` | Creates a new OTC order, locking collateral in **$OTCL tokens**. |
| `fill_order(order_id, fill_quantity)` | Matches an existing OTC limit order with a liquidity provider. |
| `cancel_order(order_id)` | Cancels an open order and returns remaining collateral. |
| `expire_order(order_id)` | Auto-expires an order when its **TTL (time-to-live)** is reached. |
| `approve_order(order_id)` | **Multisig traders** approve an order before execution. |

### 2️⃣ Staking & Priority Execution

| Instruction | Description |
|------------|------------|
| `stake_tokens(amount)` | Locks **$OTCL tokens** in the staking contract for fee discounts & priority execution. |
| `withdraw_stake(amount)` | Withdraws staked tokens, removing VIP perks. |
| `get_stake_tier(trader)` | Retrieves a trader’s **VIP tier** (higher = better execution speed). |

### 3️⃣ Anti-Frontrunning (Commit-Reveal)

| Instruction | Description |
|------------|------------|
| `commit_order(order_id, commit_hash)` | Stores a hashed order to prevent frontrunning attacks. |
| `reveal_order(order_id, price, quantity, ttl, is_multisig, multisig_threshold)` | Reveals order details and executes if the hash matches. |

### 4️⃣ Governance & Treasury

| Instruction | Description |
|------------|------------|
| `withdraw_treasury(amount, governance_account)` | Withdraws accumulated fees from the treasury (**DAO-controlled**). |
| `update_fee_percentage(new_fee)` | Updates the **maker-taker fee model** (governance controlled). |

---
