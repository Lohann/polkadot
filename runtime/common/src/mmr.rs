// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! A pallet responsible for creating Merkle Mountain Range (MMR) leaf for current block.

use beefy_primitives::ValidatorSetId;
use sp_core::H256;
use sp_runtime::traits::Convert;
use sp_std::prelude::*;
use frame_support::{decl_module, decl_storage, RuntimeDebug,};
use pallet_mmr::primitives::LeafDataProvider;
use parity_scale_codec::{Encode, Decode};
use runtime_parachains::paras;

/// A leaf that gets added every block to the MMR constructed by [pallet_mmr].
#[derive(RuntimeDebug, PartialEq, Eq, Clone, Encode, Decode)]
pub struct MmrLeaf<BlockNumber, Hash, MerkleRoot> {
	/// Current block parent number and hash.
	pub parent_number_and_hash: (BlockNumber, Hash),
	/// A merkle root of all registered parachain heads.
	pub parachain_heads: MerkleRoot,
	/// A merkle root of the next BEEFY authority set.
	pub beefy_next_authority_set: BeefyNextAuthoritySet<MerkleRoot>,
}

/// Details of the next BEEFY authority set.
#[derive(RuntimeDebug, Default, PartialEq, Eq, Clone, Encode, Decode)]
pub struct BeefyNextAuthoritySet<MerkleRoot> {
	/// Id of the next set.
	///
	/// Id is required to correlate BEEFY signed commitments with the validator set.
	/// Light Client can easily verify that the commitment witness it is getting is
	/// produced by the latest validator set.
	pub id: ValidatorSetId,
	/// Number of validators in the set.
	///
	/// Some BEEFY Light Clients may use an interactive protocol to verify only subset
	/// of signatures. We put set length here, so that these clients can verify the minimal
	/// number of required signatures.
	pub len: u32,
	/// Merkle Root Hash build from BEEFY AuthorityIds.
	///
	/// This is used by Light Clients to confirm that the commitments are signed by the correct
	/// validator set. Light Clients using interactive protocol, might verify only subset of
	/// signatures, hence don't require the full list here (will receive inclusion proofs).
	pub root: MerkleRoot,
}

type MerkleRootOf<T> = <T as pallet_mmr::Config>::Hash;

/// The module's configuration trait.
pub trait Config: pallet_mmr::Config + paras::Config + pallet_beefy::Config {
	/// Convert BEEFY AuthorityId to a form that would end up in the Merkle Tree.
	///
	/// For instance for ECDSA (secp256k1) we want to store uncompressed public keys (65 bytes)
	/// to simplify using them on Ethereum chain, but the rest of the Substrate codebase
	/// is storing them compressed (33 bytes) for efficiency reasons.
	type BeefyAuthorityToMerkleLeaf: Convert<<Self as pallet_beefy::Config>::AuthorityId, Vec<u8>>;
}

decl_storage! {
	trait Store for Module<T: Config> as Beefy {
		/// Details of next BEEFY authority set.
		pub BeefyNextAuthorities get(fn beefy_next_authorities): BeefyNextAuthoritySet<MerkleRootOf<T>>;
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
	}
}

impl<T: Config> LeafDataProvider for Module<T> where
	MerkleRootOf<T>: From<H256>,
{
	type LeafData = MmrLeaf<
		<T as frame_system::Config>::BlockNumber,
		<T as frame_system::Config>::Hash,
		MerkleRootOf<T>,
	>;

	fn leaf_data() -> Self::LeafData {
		MmrLeaf {
			parent_number_and_hash: frame_system::Module::<T>::leaf_data(),
			parachain_heads: Module::<T>::parachain_heads_merkle_root(),
			beefy_next_authority_set: Module::<T>::beefy_next_authorities(),
		}
	}
}

impl<T: Config> Module<T> where
	MerkleRootOf<T>: From<H256>,
	<T as pallet_beefy::Config>::AuthorityId:
{
	/// Returns latest root hash of a merkle tree constructed from all registered parachain headers.
	///
	/// NOTE this does not include parathreads - only parachains are part of the merkle tree.
	///
	/// NOTE This is an initial and inefficient implementation, which re-constructs
	/// the merkle tree every block. Instead we should update the merkle root in [Self::on_initialize]
	/// call of this pallet and update the merkle tree efficiently (use on-chain storage to persist inner nodes).
	fn parachain_heads_merkle_root() -> MerkleRootOf<T> {
		let para_heads = paras::Module::<T>::parachains()
			.into_iter()
			.map(paras::Module::<T>::para_head)
			.map(|maybe_para_head| maybe_para_head.encode())
			.collect::<Vec<_>>();

		sp_io::trie::keccak_256_ordered_root(para_heads).into()
	}

	/// Returns a merkle root of a tree constructed from secp256k1 public keys of current BEEFY authority set.
	///
	/// NOTE This is an initial and inefficient implementation, which re-constructs
	/// the merkle tree every block. Instead we should update the merkle root in [on_new_session]
	/// callback, cause we know it will only change every session - in future it should be optimized
	/// to change every era instead.
	fn update_beefy_next_authority_set() {
		let id = pallet_beefy::Module::<T>::validator_set_id() + 1;
		let beefy_public_keys = pallet_beefy::Module::<T>::next_authorities()
			.into_iter()
			.map(T::BeefyAuthorityToMerkleLeaf::convert)
			.collect::<Vec<_>>();
		let len = beefy_public_keys.len() as u32;
		let root: MerkleRootOf<T> = sp_io::trie
			::keccak_256_ordered_root(beefy_public_keys).into();
		BeefyNextAuthorities::<T>::put(BeefyNextAuthoritySet {
			id,
			len,
			root,
		})
	}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Module<T> {
	type Public = <T as pallet_beefy::Config>::AuthorityId;
}

impl<T: Config> frame_support::traits::OneSessionHandler<<T as frame_system::Config>::AccountId> for Module<T> where
	T: pallet_session::Config,
	MerkleRootOf<T>: From<H256>,
{
	type Key = <T as pallet_beefy::Config>::AuthorityId;

	fn on_genesis_session<'a, I: 'a>(_validators: I) where
		I: Iterator<Item=(&'a <T as frame_system::Config>::AccountId, Self::Key)>, Self::Key: 'a
	{
		Self::update_beefy_next_authority_set();
	}

	fn on_new_session<'a, I: 'a>(_changed: bool, _validators: I, _queued_validators: I) where
		I: Iterator<Item=(&'a <T as frame_system::Config>::AccountId, Self::Key)>
	{
		Self::update_beefy_next_authority_set();
	}

	fn on_disabled(_validator_index: usize) {}
}
