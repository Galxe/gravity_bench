import os
import json
import re
from web3 import Web3
from solcx import compile_files, install_solc, set_solc_version

solc_version = "0.5.16"

def calculate_init_code_hash():
    """Calculate the correct INIT_CODE_HASH"""
    print("🔨 Calculating correct INIT_CODE_HASH...")
    
    try:
        # 1. Compile UniswapV2Pair contract
        print("   Compiling UniswapV2Pair contract...")
        
        set_solc_version(solc_version)
        pair_path = os.path.abspath('contracts/v2-core/contracts/UniswapV2Pair.sol')
        
        pair_compiled = compile_files(
            [pair_path], 
            output_values=['abi', 'bin'],
            optimize=True,
            optimize_runs=999999, 
            solc_version=solc_version
        )
        
        # 2. Get creation bytecode
        pair_contract_name = f'{pair_path}:UniswapV2Pair'
        creation_bytecode = pair_compiled[pair_contract_name]['bin']
        
        print(f"   ✅ Pair contract compiled")
        print(f"   Creation bytecode length: {len(creation_bytecode)} characters")
        print(f"   First 100 chars: {creation_bytecode[:100]}...")
        
        # 3. Calculate keccak256 hash
        w3 = Web3()
        
        # Ensure bytecode is in bytes format
        if creation_bytecode.startswith('0x'):
            bytecode_bytes = bytes.fromhex(creation_bytecode[2:])
        else:
            bytecode_bytes = bytes.fromhex(creation_bytecode)
        
        init_code_hash = w3.keccak(bytecode_bytes)
        init_code_hash_hex = "0x" + init_code_hash.hex()
        
        print(f"\n🎯 RESULT:")
        print(f"   Current INIT_CODE_HASH in Router: 0x1801eff3d98449db2848f5b37a4d857098c1397c8178cf1f1d066d9e81a40bae")
        print(f"   Correct INIT_CODE_HASH:          {init_code_hash_hex}")
        
        if init_code_hash_hex == "0x1801eff3d98449db2848f5b37a4d857098c1397c8178cf1f1d066d9e81a40bae":
            print(f"   ✅ Current hash is correct!")
            return init_code_hash_hex
        else:
            print(f"   ❌ Current hash is WRONG!")
            print(f"\n🔧 TO FIX:")
            print(f"   Replace this line in UniswapV2Library.sol:")
            print(f"   hex'1801eff3d98449db2848f5b37a4d857098c1397c8178cf1f1d066d9e81a40bae'")
            print(f"   with:")
            print(f"   hex'{init_code_hash_hex[2:]}'")
            return init_code_hash_hex
        
    except Exception as e:
        print(f"   ❌ Error calculating hash: {e}")
        import traceback
        traceback.print_exc()
        return None

def create_fixed_library_file(correct_hash):
    """Create fixed UniswapV2Library.sol file"""
    print(f"\n📝 Creating fixed UniswapV2Library.sol...")
    
    try:
        library_path = 'contracts/v2-periphery/contracts/libraries/UniswapV2Library.sol'
        backup_path = 'contracts/v2-periphery/contracts/libraries/UniswapV2Library.sol.backup'
        
        # 1. Backup original file
        if os.path.exists(library_path):
            import shutil
            shutil.copy2(library_path, backup_path)
            print(f"   ✅ Backup created: {backup_path}")
        
        # 2. Read original file
        with open(library_path, 'r') as f:
            content = f.read()
        
        # 3. Use regex to find and replace INIT_CODE_HASH
        # Pattern matches: hex'[64 character hash]' (with optional comment)
        pattern = r"hex'([a-fA-F0-9]{64})'\s*(?://.*?init\s*code\s*hash.*?)?"
        new_hash = correct_hash[2:]  # Remove 0x prefix
        
        # Find current hash
        match = re.search(pattern, content, re.IGNORECASE)
        if match:
            old_hash = match.group(1)
            print(f"   Found current hash: {old_hash}")
            print(f"   Will replace with:  {new_hash}")
            
            # Replace with new hash, preserving the comment
            replacement = f"hex'{new_hash}' // init code hash"
            new_content = re.sub(pattern, replacement, content, flags=re.IGNORECASE)
            
            # 4. Write fixed file
            with open(library_path, 'w') as f:
                f.write(new_content)
            
            print(f"   ✅ UniswapV2Library.sol updated with correct INIT_CODE_HASH")
            print(f"   Old: {old_hash}")
            print(f"   New: {new_hash}")
            return True
        else:
            print(f"   ❌ Could not find init code hash pattern in file")
            print(f"   Looking for pattern: hex'[64 hex chars]'")
            print(f"   Please manually replace the hash in {library_path}")
            
            # Show what we're looking for
            print(f"   Expected pattern examples:")
            print(f"   - hex'96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f'")
            print(f"   - hex'1801eff3d98449db2848f5b37a4d857098c1397c8178cf1f1d066d9e81a40bae' // init code hash")
            return False
        
    except Exception as e:
        print(f"   ❌ Error creating fixed file: {e}")
        import traceback
        traceback.print_exc()
        return False

def verify_fix(correct_hash):
    """Verify the fix is correct"""
    print(f"\n✅ VERIFICATION:")
    print(f"   1. The correct INIT_CODE_HASH is: {correct_hash}")
    print(f"   2. Update contracts/v2-periphery/contracts/libraries/UniswapV2Library.sol")
    print(f"   3. Find this line:")
    print(f"      hex'1801eff3d98449db2848f5b37a4d857098c1397c8178cf1f1d066d9e81a40bae'")
    print(f"   4. Replace with:")
    print(f"      hex'{correct_hash[2:]}'")
    print(f"   5. Recompile and redeploy Router contract")
    print(f"   6. Test addLiquidityETH again")

def test_hash_verification(w3, factory_address, token_address, weth_address, actual_pair_address, correct_hash):
    """Verify the calculated hash is correct"""
    print(f"\n🧪 Verifying calculated hash...")
    
    try:
        # Manually calculate pair address
        if int(token_address, 16) < int(weth_address, 16):
            token0, token1 = token_address, weth_address
        else:
            token0, token1 = weth_address, token_address
        
        # Calculate CREATE2 address
        token_pair_hash = w3.keccak(
            bytes.fromhex(token0[2:].zfill(40) + token1[2:].zfill(40))
        )
        
        create2_input = (
            bytes.fromhex("ff") + 
            bytes.fromhex(factory_address[2:].zfill(40)) + 
            token_pair_hash + 
            bytes.fromhex(correct_hash[2:])
        )
        
        calculated_address = "0x" + w3.keccak(create2_input)[-20:].hex()
        calculated_address = w3.to_checksum_address(calculated_address)
        
        print(f"   Calculated pair: {calculated_address}")
        print(f"   Actual pair:     {actual_pair_address}")
        
        if calculated_address.lower() == actual_pair_address.lower():
            print(f"   ✅ Hash verification PASSED!")
            return True
        else:
            print(f"   ❌ Hash verification FAILED!")
            return False
            
    except Exception as e:
        print(f"   ❌ Hash verification error: {e}")
        return False

def main():
    """Main function"""
    print("🎯 UniswapV2 INIT_CODE_HASH Calculator")
    print("=" * 50)
    
    # Install Solidity compiler
    internal_install_solc(solc_version)
    
    # Calculate correct hash
    correct_hash = calculate_init_code_hash()
    
    if correct_hash:
        # Verify hash (if known pair address is available)
        w3 = Web3()
        
        # Use your contract addresses
        factory_address = "0x2BB0961D1b7f928FB3dF4d90A1A825d55e2F4e1A"
        token_address = "0x27d56ED087b2833EEf82f2d69C961e09dacc8A5C"
        weth_address = "0xD6Be736ed1A9F739d141d00DEc55e9d8852184a5"
        actual_pair_address = "0xdd07DE8AB5692DE17B01693729E62D0bccD5C666"
        
        # Verify calculated hash
        if test_hash_verification(w3, factory_address, token_address, weth_address, actual_pair_address, correct_hash):
            # Create fixed file
            if create_fixed_library_file(correct_hash):
                print(f"\n🎉 SUCCESS!")
                print(f"   UniswapV2Library.sol has been updated")
                print(f"   Now recompile and redeploy Router contract")
            else:
                verify_fix(correct_hash)
        else:
            print(f"\n⚠️  Hash verification failed, but here's the calculated hash anyway:")
            verify_fix(correct_hash)
    else:
        print(f"\n❌ Failed to calculate INIT_CODE_HASH")

def internal_install_solc(version):
    """Install specified version of Solidity compiler"""
    try:
        install_solc(version)
        set_solc_version(version)
        print(f"✅ Solc v{version} ready")
    except Exception as e:
        print(f"❌ Error with solc v{version}: {e}")

if __name__ == "__main__":
    main()
