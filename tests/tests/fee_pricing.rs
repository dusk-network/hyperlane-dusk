// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The Dusk IGP charges a transfer to the EVM chain what the relaying fees in
// deploy/testnet.env.example come to (docs/costs.md, Fee coverage):
//
//   payment (LUX) = (50,000 + DUSK_IGP_GAS_OVERHEAD) gas x EVM_GAS_PRICE_WEI
//                   / 1e18 ETH per wei x ETH_DUSK_PRICE_RATIO DUSK per ETH
//                   x 1e9 LUX per DUSK x (1 + OPERATOR_MARGIN_BPS / 10,000)
//
// deploy/testnet.sh deploys the IGP with the token exchange rate
// 10 x ETH_DUSK_PRICE_RATIO x (1 + margin), rounded up, and the IGP computes
// gas x gas price x rate / 1e10. These tests deploy the IGP with what the
// script derives from the example settings, and check its quote against the
// first formula, worked out here from the settings.
//
//   make all
//   cargo test -p hyperlane-dusk-integration-tests --test fee_pricing -- --nocapture

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use dusk_core::abi::ContractId;
use dusk_vm::ContractData;

use hyperlane_dusk_types::{message, metadata, DomainGasConfig, H256, VERSION};

mod test_session;
use test_session::TestSession;

const MAILBOX_ID: ContractId = ContractId::from_bytes([10; 32]);
const IGP_ID: ContractId = ContractId::from_bytes([15; 32]);
const DEPLOYER: [u8; 64] = [0u8; 64];
const OWNER: H256 = [0x0A; 32];
const BENEFICIARY: H256 = [0x0B; 32];
/// The IGP's TOKEN_EXCHANGE_RATE_SCALE.
const SCALE: u128 = 10_000_000_000;
const LUX_PER_DUSK: u128 = 1_000_000_000;
const WEI_PER_ETH: u128 = 1_000_000_000_000_000_000;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn igp_wasm() -> Vec<u8> {
    let dir = std::env::var_os("HYPERLANE_WASM_DIR").map_or_else(
        || repo().join("target/contract/wasm32-unknown-unknown/release"),
        PathBuf::from,
    );
    let path = dir.join("hyperlane_dusk_igp.wasm");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e} (run make all)", path.display()))
}

/// The defaults in deploy/testnet.env.example: each `NAME="${NAME:-value}"`.
fn example_settings() -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(repo().join("deploy/testnet.env.example"))
        .expect("deploy/testnet.env.example should be readable");
    let mut settings = BTreeMap::new();
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            continue;
        }
        let prefix = format!("\"${{{name}:-");
        if let Some(default) = value
            .strip_prefix(&prefix)
            .and_then(|v| v.strip_suffix("}\""))
        {
            settings.insert(name.to_owned(), default.to_owned());
        }
    }
    settings
}

/// The settings the Dusk IGP's pricing comes from.
struct Prices {
    /// DUSK per ETH, as numerator / denominator.
    ratio: (u128, u128),
    margin_bps: u128,
    evm_gas_price_wei: u64,
    gas_overhead: u64,
    evm_domain: u32,
}

impl Prices {
    fn from(settings: &BTreeMap<String, String>) -> Self {
        let get = |name: &str| {
            settings
                .get(name)
                .unwrap_or_else(|| panic!("deploy/testnet.env.example has no default for {name}"))
        };
        let ratio = get("ETH_DUSK_PRICE_RATIO");
        let (whole, fraction) = ratio.split_once('.').unwrap_or((ratio, ""));
        let denominator = 10u128.pow(u32::try_from(fraction.len()).unwrap());
        let numerator: u128 = format!("{whole}{fraction}")
            .parse()
            .expect("ETH_DUSK_PRICE_RATIO is a number");
        Self {
            ratio: (numerator, denominator),
            margin_bps: get("OPERATOR_MARGIN_BPS").parse().unwrap(),
            evm_gas_price_wei: get("EVM_GAS_PRICE_WEI").parse().unwrap(),
            gas_overhead: get("DUSK_IGP_GAS_OVERHEAD").parse().unwrap(),
            evm_domain: get("EVM_DOMAIN").parse().unwrap(),
        }
    }

    /// The IGP pricing deploy/testnet.sh derives: LUX per wei, x 1e10, with
    /// the margin, rounded up.
    fn igp_config(&self) -> DomainGasConfig {
        let (numerator, denominator) = self.ratio;
        let scaled = numerator * LUX_PER_DUSK * SCALE * (10_000 + self.margin_bps);
        let divisor = denominator * WEI_PER_ETH * 10_000;
        DomainGasConfig {
            gas_overhead: self.gas_overhead,
            token_exchange_rate: u64::try_from(scaled.div_ceil(divisor)).unwrap(),
            gas_price: self.evm_gas_price_wei,
        }
    }

    /// What `gas` EVM gas costs at these prices, in LUX, with the margin:
    /// gas x gas price in wei, in ETH, in DUSK, in LUX, rounded down.
    fn lux_for(&self, gas: u64) -> u128 {
        let (numerator, denominator) = self.ratio;
        u128::from(gas)
            * u128::from(self.evm_gas_price_wei)
            * numerator
            * LUX_PER_DUSK
            * (10_000 + self.margin_bps)
            / (denominator * WEI_PER_ETH * 10_000)
    }
}

fn deploy_igp(config: DomainGasConfig, domain: u32) -> TestSession {
    let mut session = TestSession::instantiate(vec![]);
    session
        .deploy(
            &igp_wasm(),
            ContractData::builder()
                .owner(DEPLOYER)
                .init_arg(&(MAILBOX_ID, OWNER, BENEFICIARY, vec![(domain, config)]))
                .contract_id(IGP_ID),
        )
        .expect("the IGP should accept the pricing deploy/testnet.sh derives");
    session
}

/// A warp transfer's message: the routes send no hook metadata, which the
/// IGP reads as 50,000 gas.
fn transfer_quote(session: &mut TestSession, domain: u32) -> u64 {
    let encoded = message::encode(
        VERSION,
        0,
        1_146_442_572,
        [0x01; 32],
        domain,
        [0x02; 32],
        &[0u8; 64],
    );
    session
        .direct_call::<_, u64>(IGP_ID, "quote_dispatch", &(Vec::<u8>::new(), encoded))
        .expect("the IGP should quote a transfer")
        .data
}

#[test]
fn the_dusk_igp_quotes_the_configured_fee_for_a_transfer() {
    let prices = Prices::from(&example_settings());
    let config = prices.igp_config();
    let mut session = deploy_igp(config, prices.evm_domain);
    let gas = metadata::DEFAULT_GAS_USAGE + prices.gas_overhead;

    let quote = transfer_quote(&mut session, prices.evm_domain);
    let quoted_gas: u64 = session
        .direct_call::<_, u64>(
            IGP_ID,
            "quote_gas_payment",
            &(prices.evm_domain, metadata::DEFAULT_GAS_USAGE),
        )
        .expect("the IGP should quote gas")
        .data;
    assert_eq!(quote, quoted_gas);

    // The IGP's own arithmetic, on the pricing it was given.
    let by_rate =
        u128::from(gas) * u128::from(config.gas_price) * u128::from(config.token_exchange_rate)
            / SCALE;
    assert_eq!(u128::from(quote), by_rate);

    // The fee the settings mean. Rounding the rate up adds less than one LUX
    // per 1e10 wei of gas cost.
    let expected = prices.lux_for(gas);
    let slack = u128::from(gas) * u128::from(prices.evm_gas_price_wei) / SCALE + 1;
    assert!(
        expected <= u128::from(quote) && u128::from(quote) <= expected + slack,
        "the IGP quotes {quote} LUX; the settings mean {expected}"
    );
    println!(
        "fee|dusk-to-evm|{gas} gas at {} wei, rate {}|{quote} LUX",
        config.gas_price, config.token_exchange_rate
    );

    // docs/costs.md quotes the example's defaults: 455,000 gas at 1 gwei,
    // 0.000455 ETH, is 11.375 DUSK at 25,000 DUSK per ETH, and 17.0625 DUSK
    // with the 50% margin. Change both together.
    assert_eq!(gas, 455_000);
    assert_eq!(config.token_exchange_rate, 375_000);
    assert_eq!(quote, 17_062_500_000);
}

/// What deploy/testnet.sh passes to `dusk-tx deploy-hyperlane` and
/// `set-igp-config` is the pricing these tests derive.
#[test]
fn deploy_testnet_sh_derives_the_same_pricing() {
    let prices = Prices::from(&example_settings());
    let config = prices.igp_config();
    let output = Command::new("bash")
        .arg("-c")
        .arg("bash deploy/testnet.sh fees --json | jq -r .dusk_igp.config")
        .current_dir(repo())
        .env("TESTNET_ENV", repo().join("deploy/testnet.env.example"))
        .output()
        .expect("bash should run deploy/testnet.sh");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!(
            "{}:{}:{}:{}",
            prices.evm_domain, config.gas_overhead, config.token_exchange_rate, config.gas_price
        )
    );
}

/// Other prices go through the same formula, also when the rate has to be
/// rounded up.
#[test]
fn the_quote_follows_the_formula_for_other_prices() {
    for (ratio, margin_bps, gas_price, overhead) in [
        ((30_000, 1), 0, 1_000_000_000, 200_000),
        ((12_345_678, 1_000), 2_500, 2_000_000_000, 210_000),
        ((1, 2), 300, 50_000_000_000, 200_000),
        ((250_000, 1), 10_000, 100_000_000, 174_000),
    ] {
        let prices = Prices {
            ratio,
            margin_bps,
            evm_gas_price_wei: gas_price,
            gas_overhead: overhead,
            evm_domain: 11_155_111,
        };
        let config = prices.igp_config();
        let mut session = deploy_igp(config, prices.evm_domain);
        let quote = transfer_quote(&mut session, prices.evm_domain);
        let gas = metadata::DEFAULT_GAS_USAGE + overhead;
        let expected = prices.lux_for(gas);
        let slack = u128::from(gas) * u128::from(gas_price) / SCALE + 1;
        assert!(
            expected <= u128::from(quote) && u128::from(quote) <= expected + slack,
            "ratio {ratio:?}, margin {margin_bps}: the IGP quotes {quote} LUX; the settings mean {expected}"
        );
    }
}
