// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice EIP-7702 delegation target: batch ETH transfers in the EOA context.
/// When an EOA sets code to this contract and is called with multiSend,
/// value is spent from the EOA balance (address(this) == EOA).
///
/// receive/fallback are required so other delegated EOAs can still accept
/// plain ETH value transfers from multiSend (empty calldata must not revert).
///
/// multiSend is onlySelf: without `msg.sender == address(this)`, any third
/// party could call a delegated EOA and drain its ETH.
contract BatchExecutor {
    receive() external payable {}

    fallback() external payable {}

    /// @notice Transfer ETH to multiple recipients.
    /// @dev Must be called as the delegated EOA (to == EOA after set-code),
    ///      and only by the EOA itself (msg.sender == address(this)).
    function multiSend(address[] calldata recipients, uint256[] calldata amounts) external payable {
        require(msg.sender == address(this), "only self");
        uint256 n = recipients.length;
        require(n == amounts.length, "len");
        for (uint256 i = 0; i < n; ) {
            (bool ok, ) = recipients[i].call{value: amounts[i]}("");
            require(ok, "send");
            unchecked {
                ++i;
            }
        }
    }
}
