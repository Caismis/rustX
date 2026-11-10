import {useTranslation} from '../../../locale/react';
export function ConfigUnavailable() {
  const tx = useTranslation();
  return <p role="status">{tx('settings:extension-detail.structured-editing-of-this-source-s-rustx-toml-is-unavailable-be')}</p>;
}
