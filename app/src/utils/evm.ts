import {
  type Address,
  type PublicClient,
  type WalletClient,
  type Chain,
  type Transport,
  type HDAccount,
  encodeAbiParameters,
  encodeFunctionData,
  keccak256,
} from 'viem';
import {
  POAStakeRegistryABI,
  POAStakeRegistryBytecode,
  TransparentUpgradeableProxyBytecode,
  type RegistryInfo,
  type Vector,
  type DeployResult,
} from '../contracts/POAStakeRegistry';

/**
 * Deploy a new POAStakeRegistry with proxy pattern
 * In OZ v5, TransparentUpgradeableProxy deploys its own ProxyAdmin internally.
 * Deploys: Implementation -> TransparentUpgradeableProxy (which creates ProxyAdmin)
 */
export async function deployPOARegistry(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  thresholdWeight: bigint,
  quorumNumerator: bigint,
  quorumDenominator: bigint,
  onProgress?: (step: string) => void
): Promise<DeployResult> {
  const account = walletClient.account;
  if (!account) throw new Error('Wallet client has no account');

  onProgress?.('Deploying Implementation...');

  // 1. Deploy POAStakeRegistry implementation
  const implHash = await walletClient.deployContract({
    abi: POAStakeRegistryABI,
    bytecode: POAStakeRegistryBytecode,
  });

  const implReceipt = await publicClient.waitForTransactionReceipt({
    hash: implHash,
  });

  if (!implReceipt.contractAddress) {
    throw new Error('Implementation deployment failed');
  }
  const implementationAddress = implReceipt.contractAddress;

  onProgress?.('Deploying Proxy...');

  // 2. Deploy TransparentUpgradeableProxy
  // In OZ v5, constructor is: _logic, initialOwner (for internal ProxyAdmin), _data
  // The proxy will deploy its own ProxyAdmin with initialOwner as owner
  const initializeData = encodeFunctionData({
    abi: POAStakeRegistryABI,
    functionName: 'initialize',
    args: [account.address, thresholdWeight, quorumNumerator, quorumDenominator],
  });

  const proxyInitCode = (TransparentUpgradeableProxyBytecode +
    encodeAbiParameters(
      [{ type: 'address' }, { type: 'address' }, { type: 'bytes' }],
      [implementationAddress, account.address, initializeData]
    ).slice(2)) as `0x${string}`;

  const proxyHash = await walletClient.deployContract({
    abi: [],
    bytecode: proxyInitCode,
  });

  const proxyReceipt = await publicClient.waitForTransactionReceipt({
    hash: proxyHash,
  });

  if (!proxyReceipt.contractAddress) {
    throw new Error('Proxy deployment failed');
  }
  const proxyAddress = proxyReceipt.contractAddress;

  // ProxyAdmin is deployed internally by the proxy - we don't have its address directly
  // but it's not needed for normal operations
  return {
    proxyAdminAddress: '0x0000000000000000000000000000000000000000' as `0x${string}`,
    implementationAddress,
    proxyAddress,
  };
}

/**
 * Connect to an existing registry and fetch its info
 */
export async function connectToRegistry(
  publicClient: PublicClient<Transport, Chain>,
  address: Address
): Promise<RegistryInfo> {
  const [owner, totalWeight, thresholdWeight, quorum, serviceUri] = await Promise.all([
    publicClient.readContract({
      address,
      abi: POAStakeRegistryABI,
      functionName: 'owner',
    }),
    publicClient.readContract({
      address,
      abi: POAStakeRegistryABI,
      functionName: 'getLastCheckpointTotalWeight',
    }),
    publicClient.readContract({
      address,
      abi: POAStakeRegistryABI,
      functionName: 'getLastCheckpointThresholdWeight',
    }),
    publicClient.readContract({
      address,
      abi: POAStakeRegistryABI,
      functionName: 'getLastCheckpointQuorum',
    }),
    publicClient.readContract({
      address,
      abi: POAStakeRegistryABI,
      functionName: 'getServiceURI',
    }),
  ]);

  return {
    owner: owner as Address,
    totalWeight: totalWeight as bigint,
    thresholdWeight: thresholdWeight as bigint,
    quorumNumerator: quorum[0] as bigint,
    quorumDenominator: quorum[1] as bigint,
    serviceUri: serviceUri as string,
  };
}

/**
 * Fetch vectors from registry events.
 * Pass knownAddresses to ensure specific addresses are checked even if
 * event logs haven't been indexed yet (e.g. right after registration).
 */
export async function fetchOperators(
  publicClient: PublicClient<Transport, Chain>,
  registryAddress: Address,
  fromBlock?: bigint,
  knownAddresses?: Address[]
): Promise<Vector[]> {
  // Get all OperatorRegistered and OperatorDeregistered events
  const [registerLogs, deregisterLogs] = await Promise.all([
    publicClient.getLogs({
      address: registryAddress,
      event: {
        type: 'event',
        name: 'OperatorRegistered',
        inputs: [{ name: 'vector', type: 'address', indexed: true }],
      },
      fromBlock: fromBlock ?? 0n,
      toBlock: 'latest',
    }),
    publicClient.getLogs({
      address: registryAddress,
      event: {
        type: 'event',
        name: 'OperatorDeregistered',
        inputs: [{ name: 'vector', type: 'address', indexed: true }],
      },
      fromBlock: fromBlock ?? 0n,
      toBlock: 'latest',
    }),
  ]);

  // Track registered vectors
  const operatorSet = new Set<Address>();
  const deregisteredSet = new Set<Address>();

  for (const log of registerLogs) {
    const vector = log.args.vector as Address;
    operatorSet.add(vector);
  }

  for (const log of deregisterLogs) {
    const vector = log.args.vector as Address;
    deregisteredSet.add(vector);
  }

  // Include known addresses so they get checked via contract reads
  // even if event logs haven't been indexed yet
  if (knownAddresses) {
    for (const addr of knownAddresses) {
      operatorSet.add(addr);
    }
  }

  // Get currently registered vectors
  const currentOperators: Address[] = [];
  for (const op of operatorSet) {
    if (!deregisteredSet.has(op)) {
      currentOperators.push(op);
    }
  }

  // Fetch details for each vector
  const vectors: Vector[] = await Promise.all(
    currentOperators.map(async (address) => {
      const [isRegistered, weight, signingKey] = await Promise.all([
        publicClient.readContract({
          address: registryAddress,
          abi: POAStakeRegistryABI,
          functionName: 'operatorRegistered',
          args: [address],
        }),
        publicClient.readContract({
          address: registryAddress,
          abi: POAStakeRegistryABI,
          functionName: 'getOperatorWeight',
          args: [address],
        }),
        publicClient.readContract({
          address: registryAddress,
          abi: POAStakeRegistryABI,
          functionName: 'getLatestOperatorSigningKey',
          args: [address],
        }),
      ]);

      return {
        address,
        weight: weight as bigint,
        signingKey: signingKey as Address,
        isRegistered: isRegistered as boolean,
      };
    })
  );

  return vectors.filter((op) => op.isRegistered);
}

/**
 * Register a new vector
 */
export async function registerOperator(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  operatorAddress: Address,
  weight: bigint
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'registerOperator',
    args: [operatorAddress, weight],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Deregister an vector
 */
export async function deregisterOperator(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  operatorAddress: Address
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'deregisterOperator',
    args: [operatorAddress],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Update an vector's weight
 */
export async function updateOperatorWeight(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  operatorAddress: Address,
  weight: bigint
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'updateOperatorWeight',
    args: [operatorAddress, weight],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Set the service URI
 */
export async function setServiceURI(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  uri: string
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'setServiceURI',
    args: [uri],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Update the stake threshold
 */
export async function updateStakeThreshold(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  thresholdWeight: bigint
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'updateStakeThreshold',
    args: [thresholdWeight],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Update the quorum
 */
export async function updateQuorum(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  numerator: bigint,
  denominator: bigint
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'updateQuorum',
    args: [numerator, denominator],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Transfer ownership of the registry
 */
export async function transferOwnership(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  newOwner: Address
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'transferOwnership',
    args: [newOwner],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Update an vector's signing key.
 * Must be called by the vector themselves.
 * The signing key signs keccak256(abi.encode(operatorAddress)) as a raw hash (no EIP-191 prefix).
 */
export async function updateSigningKey(
  publicClient: PublicClient<Transport, Chain>,
  walletClient: WalletClient<Transport, Chain, HDAccount>,
  registryAddress: Address,
  signingKeyAddress: Address,
  signingKeySignature: `0x${string}`
): Promise<`0x${string}`> {
  const hash = await walletClient.writeContract({
    address: registryAddress,
    abi: POAStakeRegistryABI,
    functionName: 'updateOperatorSigningKey',
    args: [signingKeyAddress, signingKeySignature],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

/**
 * Create the raw signature needed for updateOperatorSigningKey.
 * Signs keccak256(abi.encode(operatorAddress)) with the signing key (no EIP-191 prefix).
 */
export async function createSigningKeySignature(
  signingKeyAccount: HDAccount,
  operatorAddress: Address
): Promise<`0x${string}`> {
  const messageHash = keccak256(
    encodeAbiParameters([{ type: 'address' }], [operatorAddress])
  );
  return signingKeyAccount.sign({ hash: messageHash });
}
