// SPDX-License-Identifier: MIT
pragma solidity ^0.8.27;

import {IWarpDriveServiceHandler} from "../interfaces/IWarpDriveServiceHandler.sol";
import {IWarpDriveServiceManager} from "../interfaces/IWarpDriveServiceManager.sol";
import {ISimpleTrigger} from "./ISimpleTrigger.sol";
import {ISimpleSubmit} from "./ISimpleSubmit.sol";

/**
 * @title SimpleSubmit
 * @author Lay3r Labs
 * @notice Contract for the simple submit contract
 * @dev This contract implements the IWarpDriveServiceHandler and ISimpleSubmit interfaces
 */
contract SimpleSubmit is IWarpDriveServiceHandler, ISimpleSubmit {
    IWarpDriveServiceManager private immutable _SERVICE_MANAGER;

    /// @notice Mapping from trigger ID to valid triggers
    mapping(ISimpleTrigger.TriggerId => bool) public validTriggers;
    /// @notice Mapping from trigger ID to signed data
    mapping(ISimpleTrigger.TriggerId => ISimpleSubmit.SignedData) public signedDatas;

    /**
     * @notice Constructor
     * @param serviceManager The service manager
     */
    constructor(
        IWarpDriveServiceManager serviceManager
    ) {
        _SERVICE_MANAGER = serviceManager;
    }

    /// @inheritdoc IWarpDriveServiceHandler
    function handleSignedEnvelope(
        IWarpDriveServiceHandler.Envelope calldata envelope,
        IWarpDriveServiceHandler.SignatureData calldata signatureData
    ) external {
        _SERVICE_MANAGER.validate(envelope, signatureData);

        ISimpleSubmit.DataWithId memory dataWithId =
            abi.decode(envelope.payload, (ISimpleSubmit.DataWithId));

        signedDatas[dataWithId.triggerId] = ISimpleSubmit.SignedData({
            data: dataWithId.data,
            signatureData: signatureData,
            envelope: envelope
        });

        validTriggers[dataWithId.triggerId] = true;
    }

    /**
     * @notice Checks if a trigger ID is valid
     * @param triggerId The trigger ID to check
     * @return True if the trigger ID is valid, false otherwise
     */
    function isValidTriggerId(
        ISimpleTrigger.TriggerId triggerId
    ) external view returns (bool) {
        return validTriggers[triggerId];
    }

    /// @inheritdoc ISimpleSubmit
    function getSignedData(
        ISimpleTrigger.TriggerId triggerId
    ) external view returns (ISimpleSubmit.SignedData memory signedData) {
        signedData = signedDatas[triggerId];
    }

    /// @inheritdoc ISimpleSubmit
    function getDataWithId(
        ISimpleTrigger.TriggerId triggerId
    ) external view returns (ISimpleSubmit.DataWithId memory dataWithId) {
        ISimpleSubmit.SignedData memory signedData = signedDatas[triggerId];
        dataWithId = ISimpleSubmit.DataWithId({triggerId: triggerId, data: signedData.data});
    }

    /// @inheritdoc IWarpDriveServiceHandler
    function getServiceManager() external view returns (address) {
        return address(_SERVICE_MANAGER);
    }
}
