use crate as pallet_gas_topup;
use frame_support::{assert_noop, assert_ok, parameter_types, PalletId};
use frame_system as system;
use crate::BoundedVec;
use sp_core::H256;
use sp_runtime::{traits::{BlakeTwo256, IdentityLookup}, BuildStorage};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sp_io::hashing::keccak_256;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Signet: pallet_signet,
        BuildEvmTx: pallet_build_evm_tx,
        GasTopup: pallet_gas_topup,
    }
);

parameter_types! { pub const BlockHashCount: u64 = 250; }
impl system::Config for Test {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = ();
    type BlockLength = ();
    type DbWeight = ();
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    type Nonce = u64;
    type Hash = H256;
    type Hashing = BlakeTwo256;
    type AccountId = u64;
    type Lookup = IdentityLookup<Self::AccountId>;
    type Block = frame_system::mocking::MockBlock<Test>;
    type RuntimeEvent = RuntimeEvent;
    type BlockHashCount = BlockHashCount;
    type Version = ();
    type PalletInfo = PalletInfo;
    type AccountData = pallet_balances::AccountData<u128>;
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = ();
    type OnSetCode = ();
    type MaxConsumers = frame_support::traits::ConstU32<16>;
    type RuntimeTask = ();
    type SingleBlockMigrations = ();
    type MultiBlockMigrator = ();
    type PreInherents = ();
    type PostInherents = ();
    type PostTransactions = ();
}

parameter_types! { pub const ExistentialDeposit: u128 = 1; }
impl pallet_balances::Config for Test {
    type Balance = u128;
    type DustRemoval = ();
    type RuntimeEvent = RuntimeEvent;
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
    type WeightInfo = ();
    type MaxLocks = ();
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    type FreezeIdentifier = ();
    type MaxFreezes = ();
    type RuntimeHoldReason = ();
    type RuntimeFreezeReason = ();
}

parameter_types! {
    pub const SignetPalletId: PalletId = PalletId(*b"py/signt");
    pub const MaxChainIdLength: u32 = 128;
}
impl pallet_signet::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type PalletId = SignetPalletId;
    type MaxChainIdLength = MaxChainIdLength;
    type WeightInfo = ();
}

parameter_types! { pub const MaxDataLength: u32 = 1024; }
impl pallet_build_evm_tx::Config for Test { type MaxDataLength = MaxDataLength; }

parameter_types! { pub const GasTopupPalletId: PalletId = PalletId(*b"py/gastp"); }
impl pallet_gas_topup::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type VaultPalletId = GasTopupPalletId;
}

fn new_test_ext() -> sp_io::TestExternalities {
    let t = system::GenesisConfig::<Test>::default().build_storage().unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);
    });
    ext
}

#[test]
fn initialize_works() {
    new_test_ext().execute_with(|| {
        let mpc = [1u8; 20];
        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(1), mpc));
        assert_eq!(pallet_gas_topup::ConfigData::<Test>::get(), Some(mpc));
        System::assert_last_event(RuntimeEvent::GasTopup(pallet_gas_topup::Event::<Test>::Initialized { mpc }));
    });
}

#[test]
fn initialize_twice_fails() {
    new_test_ext().execute_with(|| {
        let mpc = [2u8; 20];
        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(1), mpc));
        assert_noop!(
            GasTopup::initialize(RuntimeOrigin::signed(2), [3u8; 20]),
            pallet_gas_topup::Error::<Test>::AlreadyInitialized
        );
        assert_eq!(pallet_gas_topup::ConfigData::<Test>::get(), Some(mpc));
    });
}

#[test]
fn request_fund_fails_when_not_initialized() {
    new_test_ext().execute_with(|| {
        let requester = 10u64;
        let _ = <Balances as frame_support::traits::Currency<_>>::deposit_creating(&requester, 1_000_000);
        assert_noop!(
            GasTopup::request_fund(
                RuntimeOrigin::signed(requester),
                [5u8; 20],
                1_000_000_000_000_000u128,
                500u128,
                [7u8; 20],
                evm_tx(),
            ),
            pallet_gas_topup::Error::<Test>::NotInitialized
        );
    });
}

#[test]
fn request_fund_emits_event_stores_pending_and_deducts_balances() {
    new_test_ext().execute_with(|| {
        let admin = 1u64;
        let requester = 2u64;
        let mpc = [9u8; 20];
        let to = [5u8; 20];
        let faucet = [7u8; 20];
        let amount_wei = 1_000_000_000_000_000u128;
        let pay_amount: u128 = 1_234;

        let _ = <Balances as frame_support::traits::Currency<_>>::deposit_creating(&requester, 2_000_000);
        let _ = pallet_signet::Pallet::<Test>::initialize(
            RuntimeOrigin::signed(admin),
            admin,
            100,
            BoundedVec::try_from(b"sig-chain".to_vec()).unwrap(),
        );

        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(admin), mpc));

        let pallet_acc = GasTopup::account_id();
        let bal_req_before = <Balances as frame_support::traits::Currency<_>>::free_balance(&requester);
        let bal_pal_before = <Balances as frame_support::traits::Currency<_>>::free_balance(&pallet_acc);
        let sig_deposit = pallet_signet::Pallet::<Test>::signature_deposit();

        assert_ok!(GasTopup::request_fund(
            RuntimeOrigin::signed(requester),
            to,
            amount_wei,
            pay_amount,
            faucet,
            evm_tx(),
        ));

        let events = System::events();
        let mut req_id_opt: Option<[u8; 32]> = None;
        for e in events.iter() {
            if let RuntimeEvent::GasTopup(pallet_gas_topup::Event::FundRequested { request_id, requester: who, to: t, amount_wei: a, pay_amount: p }) = &e.event {
                assert_eq!(*who, requester);
                assert_eq!(*t, to);
                assert_eq!(*a, amount_wei);
                assert_eq!(*p, pay_amount.into());
                req_id_opt = Some(*request_id);
            }
        }
        let request_id = req_id_opt.expect("FundRequested not found");

        let pending = pallet_gas_topup::Pending::<Test>::get(&request_id).expect("pending missing");
        assert_eq!(pending.requester, requester);
        assert_eq!(pending.to, to);
        assert_eq!(pending.amount_wei, amount_wei);
        assert_eq!(pending.pay_amount, pay_amount);

        assert!(events.iter().any(|e| matches!(
            e.event,
            RuntimeEvent::Signet(pallet_signet::Event::SignRespondRequested { .. })
        )));

        let bal_req_after = <Balances as frame_support::traits::Currency<_>>::free_balance(&requester);
        let bal_pal_after = <Balances as frame_support::traits::Currency<_>>::free_balance(&pallet_acc);
				println!("{}", sig_deposit);
        assert_eq!(bal_req_before - bal_req_after, sig_deposit + pay_amount);
        assert_eq!(bal_pal_after - bal_pal_before, pay_amount);
    });
}

#[test]
fn request_fund_duplicate_request_id_fails() {
    new_test_ext().execute_with(|| {
        let admin = 1u64;
        let requester = 2u64;
        let mpc = [9u8; 20];
        let to = [5u8; 20];
        let faucet = [7u8; 20];
        let amount_wei = 1_000_000_000_000_000u128;
        let pay_amount: u128 = 777;

        let _ = <Balances as frame_support::traits::Currency<_>>::deposit_creating(&requester, 2_000_000);
        let _ = pallet_signet::Pallet::<Test>::initialize(
            RuntimeOrigin::signed(admin),
            admin,
            100,
            BoundedVec::try_from(b"sig-chain".to_vec()).unwrap(),
        );
        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(admin), mpc));

        let tx = evm_tx();

        assert_ok!(GasTopup::request_fund(
            RuntimeOrigin::signed(requester),
            to,
            amount_wei,
            pay_amount,
            faucet,
            tx.clone(),
        ));

        assert_noop!(
            GasTopup::request_fund(
                RuntimeOrigin::signed(requester),
                to,
                amount_wei,
                pay_amount,
                faucet,
                tx,
            ),
            pallet_gas_topup::Error::<Test>::DuplicateRequest
        );
    });
}

#[test]
fn respond_fund_succeeds_and_keeps_funds_on_true() {
    new_test_ext().execute_with(|| {
        let admin = 1u64;
        let user = 2u64;
        let mpc = pk_to_addr(&test_pk());
        let pay_amount: u128 = 555;


        let _ = <Balances as frame_support::traits::Currency<_>>::deposit_creating(&user, 2_000_000);
        let _ = pallet_signet::Pallet::<Test>::initialize(RuntimeOrigin::signed(admin), admin, 100, BoundedVec::try_from(b"sig".to_vec()).unwrap());
        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(admin), mpc));

				let sig_deposit = pallet_signet::Pallet::<Test>::signature_deposit();

        let bal_user_before = <Balances as frame_support::traits::Currency<_>>::free_balance(&user);
        assert_ok!(GasTopup::request_fund(RuntimeOrigin::signed(user), [5u8; 20], 1_000_000_000_000_000, pay_amount, [7u8; 20], evm_tx()));
        let req_id = get_last_request_id_from_events();

        let out = vec![1u8];
        let sig = sign_hash32(hash_msg(req_id, &out));
        assert_ok!(GasTopup::respond_fund(RuntimeOrigin::signed(user), req_id, BoundedVec::try_from(out).unwrap(), sig));

        assert!(pallet_gas_topup::Pending::<Test>::get(&req_id).is_none());
        System::assert_has_event(RuntimeEvent::GasTopup(pallet_gas_topup::Event::FundSucceeded { request_id: req_id }));
        let bal_user_after = <Balances as frame_support::traits::Currency<_>>::free_balance(&user);
				println!("{} {}", sig_deposit, pay_amount);
        assert_eq!(bal_user_before - bal_user_after, sig_deposit + pay_amount);
    });
}

#[test]
fn respond_fund_refunds_on_false() {
    new_test_ext().execute_with(|| {
        let admin = 1u64;
        let user = 2u64;
        let mpc = pk_to_addr(&test_pk());
        let pay_amount: u128 = 777;

        let _ = <Balances as frame_support::traits::Currency<_>>::deposit_creating(&user, 2_000_000);
        let _ = pallet_signet::Pallet::<Test>::initialize(RuntimeOrigin::signed(admin), admin, 100, BoundedVec::try_from(b"sig".to_vec()).unwrap());
        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(admin), mpc));

        assert_ok!(GasTopup::request_fund(RuntimeOrigin::signed(user), [5u8; 20], 2_000_000_000_000_000, pay_amount, [7u8; 20], evm_tx()));
        let req_id = get_last_request_id_from_events();

        let bal_user_mid = <Balances as frame_support::traits::Currency<_>>::free_balance(&user);
        let out = vec![0u8];
        let sig = sign_hash32(hash_msg(req_id, &out));
        assert_ok!(GasTopup::respond_fund(RuntimeOrigin::signed(user), req_id, BoundedVec::try_from(out).unwrap(), sig));

        System::assert_has_event(RuntimeEvent::GasTopup(pallet_gas_topup::Event::FundFailed { request_id: req_id, refunded: pay_amount }));
        let bal_user_after = <Balances as frame_support::traits::Currency<_>>::free_balance(&user);
        assert_eq!(bal_user_after, bal_user_mid + pay_amount);
        assert!(pallet_gas_topup::Pending::<Test>::get(&req_id).is_none());
    });
}

#[test]
fn respond_fund_invalid_signature_fails() {
    new_test_ext().execute_with(|| {
        let admin = 1u64;
        let user = 2u64;
        let mpc = pk_to_addr(&test_pk());
        let pay_amount: u128 = 111;

        let _ = <Balances as frame_support::traits::Currency<_>>::deposit_creating(&user, 2_000_000);
        let _ = pallet_signet::Pallet::<Test>::initialize(RuntimeOrigin::signed(admin), admin, 100, BoundedVec::try_from(b"sig".to_vec()).unwrap());
        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(admin), mpc));

        assert_ok!(GasTopup::request_fund(RuntimeOrigin::signed(user), [9u8; 20], 3_000_000_000_000_000, pay_amount, [7u8; 20], evm_tx()));
        let req_id = get_last_request_id_from_events();

        let out = vec![1u8];
        let wrong_hash = hash_msg([3u8; 32], &out);
        let sig = sign_hash32(wrong_hash);

        assert_noop!(
            GasTopup::respond_fund(RuntimeOrigin::signed(user), req_id, BoundedVec::try_from(out).unwrap(), sig),
            pallet_gas_topup::Error::<Test>::InvalidSigner
        );
        assert!(pallet_gas_topup::Pending::<Test>::get(&req_id).is_some());
    });
}

#[test]
fn respond_fund_not_found_fails() {
    new_test_ext().execute_with(|| {
        let admin = 1u64;
        let user = 2u64;
        let mpc = pk_to_addr(&test_pk());
        let _ = pallet_signet::Pallet::<Test>::initialize(RuntimeOrigin::signed(admin), admin, 100, BoundedVec::try_from(b"sig".to_vec()).unwrap());
        assert_ok!(GasTopup::initialize(RuntimeOrigin::signed(admin), mpc));

        let out = vec![1u8];
        let sig = sign_hash32(hash_msg([9u8; 32], &out));

        assert_noop!(
            GasTopup::respond_fund(RuntimeOrigin::signed(user), [9u8; 32], BoundedVec::try_from(out).unwrap(), sig),
            pallet_gas_topup::Error::<Test>::NotFound
        );
    });
}

fn evm_tx() -> pallet_gas_topup::EvmTx {
    pallet_gas_topup::EvmTx {
        value: 0,
        gas_limit: 100_000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        nonce: 0,
        chain_id: 11155111,
    }
}

fn test_sk() -> SecretKey { SecretKey::from_slice(&[42u8; 32]).unwrap() }
fn test_pk() -> PublicKey { PublicKey::from_secret_key(&Secp256k1::new(), &test_sk()) }
fn pk_to_addr(pk: &PublicKey) -> [u8; 20] {
    let u = pk.serialize_uncompressed();
    let h = keccak_256(&u[1..]);
    let mut a = [0u8; 20]; a.copy_from_slice(&h[12..]); a
}
fn sign_hash32(h: [u8; 32]) -> pallet_signet::Signature {
    let secp = Secp256k1::new();
    let sig = secp.sign_ecdsa_recoverable(&Message::from_slice(&h).unwrap(), &test_sk());
    let (rid, bytes) = sig.serialize_compact();
    let mut r = [0u8; 32]; r.copy_from_slice(&bytes[..32]);
    let mut s = [0u8; 32]; s.copy_from_slice(&bytes[32..]);
    pallet_signet::Signature {
        big_r: pallet_signet::AffinePoint { x: r, y: [0u8; 32] },
        s,
        recovery_id: rid.to_i32() as u8,
    }
}

fn get_last_request_id_from_events() -> [u8; 32] {
    let mut id = [0u8; 32];
    for e in System::events().iter().rev() {
        if let RuntimeEvent::GasTopup(pallet_gas_topup::Event::FundRequested { request_id, .. }) = e.event {
            id = request_id; break
        }
    }
    id
}
fn hash_msg(req_id: [u8; 32], out: &[u8]) -> [u8; 32] {
    let mut v = Vec::with_capacity(32 + out.len());
    v.extend_from_slice(&req_id);
    v.extend_from_slice(out);
    keccak_256(&v)
}
