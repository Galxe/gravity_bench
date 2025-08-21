#!/usr/bin/env python3
import os
import re
import shutil
import tempfile
from web3 import Web3

# Set up custom temp directory before importing solcx
HOME_DIR = os.path.expanduser("~")
CUSTOM_TEMP_DIR = os.path.join(HOME_DIR, ".tmp_solcx")
os.makedirs(CUSTOM_TEMP_DIR, exist_ok=True)

# Set all temp-related environment variables
os.environ['TMPDIR'] = CUSTOM_TEMP_DIR
os.environ['TMP'] = CUSTOM_TEMP_DIR  
os.environ['TEMP'] = CUSTOM_TEMP_DIR
os.environ['TEMPDIR'] = CUSTOM_TEMP_DIR

# Set Python's tempfile module to use our custom directory
tempfile.tempdir = CUSTOM_TEMP_DIR

# Now import solcx after setting up the temp directory
from solcx import compile_files, install_solc, set_solc_version, get_solcx_install_folder

# --- Configuration ---
SOLC_VERSION = "0.5.16"
UNISWAP_V2_PAIR_PATH = 'contracts/v2-core/contracts/UniswapV2Pair.sol'
UNISWAP_V2_LIBRARY_PATH = 'contracts/v2-periphery/contracts/libraries/UniswapV2Library.sol'

# Get the default solc installation directory (defaults to ~/.solcx)
SOLC_INSTALL_DIR = get_solcx_install_folder()

def install_solc_version(version):
    """Install and set the specified version of the Solidity compiler."""
    try:
        print(f"📦 Installing solc v{version} to {SOLC_INSTALL_DIR}...")
        print(f"   Using temp directory: {CUSTOM_TEMP_DIR}")
        os.makedirs(SOLC_INSTALL_DIR, exist_ok=True)
        install_solc(version)
        set_solc_version(version)
        print(f"✅ Solc v{version} is ready.")
    except Exception as e:
        print(f"❌ Error setting up solc v{version}: {e}")
        exit(1)

def get_current_init_code_hash(library_path):
    """Reads the library file and extracts the current INIT_CODE_HASH."""
    print(f"🔍 Reading current INIT_CODE_HASH from {library_path}...")
    try:
        with open(library_path, 'r') as f:
            content = f.read()

        # This more specific pattern looks for the 'init code hash' comment
        # to ensure we're targeting the correct value.
        pattern = r"hex'([a-fA-F0-9]{64})'\s*//.*init\s*code\s*hash"
        match = re.search(pattern, content, re.IGNORECASE)

        if match:
            current_hash = "0x" + match.group(1)
            print(f"   Found current hash: {current_hash}")
            return current_hash
        else:
            print("   ⚠️ Could not find the specific INIT_CODE_HASH pattern (with comment) in the file.")
            print("      Please ensure the line looks like: hex'...' // init code hash")
            return None
    except FileNotFoundError:
        print(f"   ❌ Error: Library file not found at {library_path}")
        return None
    except Exception as e:
        print(f"   ❌ Error reading file: {e}")
        return None

def calculate_new_init_code_hash():
    """
    Compiles the UniswapV2Pair contract and calculates its creation code hash.
    """
    print("🔨 Calculating new INIT_CODE_HASH from contract bytecode...")
    try:
        pair_path = os.path.abspath(UNISWAP_V2_PAIR_PATH)
        if not os.path.exists(pair_path):
            print(f"   ❌ Error: Contract file not found at {pair_path}")
            return None

        compiled_sol = compile_files(
            [pair_path],
            output_values=['bin'],
            optimize=True,
            optimize_runs=999999,
            solc_version=SOLC_VERSION
        )

        contract_key = f'{pair_path}:UniswapV2Pair'
        creation_bytecode = compiled_sol[contract_key]['bin']

        w3 = Web3()
        bytecode_bytes = bytes.fromhex(creation_bytecode.replace('0x', ''))
        new_hash = "0x" + w3.keccak(bytecode_bytes).hex()

        print(f"   ✅ New INIT_CODE_HASH calculated: {new_hash}")
        return new_hash

    except Exception as e:
        print(f"   ❌ Error calculating new hash: {e}")
        import traceback
        traceback.print_exc()
        return None

def update_library_file(library_path, new_hash):
    """
    Updates the INIT_CODE_HASH in the library file with the new hash.
    It creates a backup of the original file first.
    """
    print(f"📝 Updating INIT_CODE_HASH in {library_path}...")
    backup_path = library_path + '.bak'

    try:
        shutil.copy2(library_path, backup_path)
        print(f"   ✅ Backup created at: {backup_path}")

        with open(library_path, 'r') as f:
            content = f.read()

        # This more specific pattern finds the hex string and preserves any trailing comment.
        pattern = r"hex'([a-fA-F0-9]{64})'(\s*//.*init\s*code\s*hash.*)?"
        new_hash_without_prefix = new_hash.replace('0x', '')

        # Build the replacement string, keeping a generic comment.
        replacement = f"hex'{new_hash_without_prefix}' // init code hash"

        new_content, num_replacements = re.subn(pattern, replacement, content, count=1, flags=re.IGNORECASE)

        if num_replacements > 0:
            with open(library_path, 'w') as f:
                f.write(new_content)
            print("   ✅ File updated successfully.")
            return True
        else:
            print("   ❌ Could not find the INIT_CODE_HASH to replace. Please check the file content.")
            return False

    except Exception as e:
        print(f"   ❌ Error updating file: {e}")
        # Restore from backup in case of error
        if os.path.exists(backup_path):
            shutil.move(backup_path, library_path)
            print("   ℹ️ Original file has been restored from backup.")
        return False

def main():
    """Main execution flow."""
    print("=" * 50)
    print("🚀 UniswapV2 INIT_CODE_HASH Refresh Script 🚀")
    print("=" * 50)

    install_solc_version(SOLC_VERSION)

    current_hash = get_current_init_code_hash(UNISWAP_V2_LIBRARY_PATH)
    if current_hash is None:
        # We can still proceed to calculate and attempt to write the hash
        print("   Proceeding to calculate the correct hash...")

    correct_hash = calculate_new_init_code_hash()

    if not correct_hash:
        print("\n❌ Aborting due to failure in hash calculation.")
        return

    print("\n" + "=" * 50)
    print("📊 Comparison")
    print(f"   Current hash in file: {current_hash or 'Not Found'}")
    print(f"   Correct calculated hash: {correct_hash}")
    print("=" * 50 + "\n")

    if current_hash and current_hash.lower() == correct_hash.lower():
        print("✅ The INIT_CODE_HASH in the library file is already correct. No changes needed.")
    else:
        if current_hash:
            print("   Mismatch detected. The file needs to be updated.")
        else:
            print("   No hash found in file. Attempting to write the new hash.")
        
        if update_library_file(UNISWAP_V2_LIBRARY_PATH, correct_hash):
            print("\n🎉 Success! The library file has been updated.")
            print("   Please recompile and redeploy your contracts.")
        else:
            print("\n❌ Failure! The library file could not be updated automatically.")
            print("   Please update it manually with the correct hash:")
            print(f"   hex'{correct_hash.replace('0x','')}'")

if __name__ == "__main__":
    main()
