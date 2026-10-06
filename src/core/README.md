# YANDI Core Module

**Migrated from:** `/home/user/net/src/core/`
**Status:** ✅ Completed
**Date:** 2025-12-23**
**Last Updated:** 2025-12-31

---

## 📊 Migration Summary

### Files Migrated:

1. ✅ **identity.rs** (207 lines → optimized)
   - Real Ed25519/X25519 cryptographic keys
   - Key save/load from `~/.yandi_keys/`
   - Sign/Verify functionality
   - IPv6 virtual address generation

2. ✅ **crypto.rs** (15 lines)
   - SHA-256 hash function

3. ✅ **config.rs** (29 lines)
   - Basic network configuration

4. ✅ **mod.rs** (12 lines)
   - Module declaration
   - Re-exports

---

## 🔧 Key Changes

### Simplified Code

**NET:** 309 lines with verbose comments
**YANDI:** 207 lines, clean and concise

Removed:
- Excessive emoji and verbose logging
- Redundant helper methods
- Russian comments

### HashId Integration

Now uses proper `HashId` newtype instead of raw arrays.

---

## 📦 API Documentation

### NodeIdentity

Cryptographic identity with Ed25519/X25519 keys.

```rust
pub struct NodeIdentity {
    pub address: HashId,                    // Node ID
    pub public_key: [u8; 32],               // X25519 public key
    private_key: [u8; 32],                  // X25519 private key
    pub signing_public_key: [u8; 32],       // Ed25519 public key
    signing_private_key: [u8; 32],          // Ed25519 private key
    _private_guard: (),                     // Prevents cloning
}
```

**Methods:**
- `new()` - Generate new identity
- `load_or_create(port)` - Load existing or create new
- `save_to_file(port)` - Save to disk
- `load_from_file(port)` - Load from disk
- `sign(data)` - Sign data with Ed25519
- `verify(data, signature)` - Verify signature
- `generate_ipv6_virtual()` - Generate IPv6 virtual address
- `exists_saved(port)` - Check if saved identity exists

---

## 📈 Usage Examples

### Create New Identity

```rust
use yandi::core::NodeIdentity;

let identity = NodeIdentity::new();
println!("Node ID: {}", identity.id());
```

### Load or Create

```rust
let identity = NodeIdentity::load_or_create(9000);
// Automatically loads existing or creates new
```

### Sign and Verify

```rust
let data = b"important message";

// Sign
let signature = identity.sign(data)?;

// Verify
if identity.verify(data, &signature) {
    println!("Signature valid!");
}
```

### Generate IPv6 Virtual Address

```rust
let ipv6 = identity.generate_ipv6_virtual();
println!("Virtual IPv6: {}", ipv6);
// Output: fc00:1234:5678::abcd1234
```

---

## 📝 Files Structure

```
src/core/
├── mod.rs         # Module declaration (12 lines)
├── identity.rs    # NodeIdentity (207 lines)
├── crypto.rs      # Hash function (15 lines)
└── config.rs      # Configuration (29 lines)

Total: 4 files, ~265 lines
```

---

## 🔐 Security Features

- Ed25519 digital signatures (production-grade)
- X25519 key exchange (production-grade)
- Secure key storage (chmod 600 on Unix)
- Private key protection (_private_guard)
- No key logging

---

**Migration Progress:** 100% complete
**Code Reduction:** From ~350 lines (NET) to ~265 lines (YANDI) = -24%
**Security:** Production-ready cryptography

✨ **Core module is ready for use!**
