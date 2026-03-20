import { MODULE_METADATA as metadataConstants } from './nest/packages/common/constants';

const metadataKeys = [
  metadataConstants.IMPORTS,
  metadataConstants.EXPORTS,
  metadataConstants.CONTROLLERS,
  metadataConstants.PROVIDERS,
];

console.log(metadataKeys.join(','));
process.exit(metadataKeys[0] === 'imports' ? 0 : 1);
