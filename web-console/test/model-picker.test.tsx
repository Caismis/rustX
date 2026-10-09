import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ModelPicker, type ModelPickerState } from '../src/app/composer/ModelPicker';
import { localeController } from '../src/locale/controller';

afterEach(() => { cleanup(); act(() => localeController.setLocale('en')); });
const choices = [{ id: 'DeepSeek/flash', profiles: [{ id: 'on', label: 'on' }] }, { id: 'vLLM/Qwen3.6', profiles: [] }];
it('search and keyboard selection retain exact model identities across live locale changes', async () => {
  const choose = vi.fn(async () => true), chosen = vi.fn();
  const state: ModelPickerState = { choices, current: 'DeepSeek/flash', disabled: false, loading: false, choose };
  render(<ModelPicker state={state} close={() => {}} chosen={chosen}/>);
  expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['DeepSeek/flash', 'DeepSeek/flash / Model default profile', 'DeepSeek/flash / on', 'vLLM/Qwen3.6']);
  expect(screen.getByText('DeepSeek')).toBeTruthy();
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'qWEN' } });
  for (const locale of ['zh', 'en'] as const) {
    act(() => localeController.setLocale(locale));
    expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['vLLM/Qwen3.6']);
    expect(choose).not.toHaveBeenCalled();
  }
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'model', model: 'vLLM/Qwen3.6' });
  expect(chosen).toHaveBeenCalledOnce();
});
it('choosing the current model preserves its configured profile without another mutation', async () => {
  const choose = vi.fn(async () => true), chosen = vi.fn(), close = vi.fn();
  render(<ModelPicker state={{ choices, current: 'DeepSeek/flash', disabled: false, loading: false, choose }} close={close} chosen={chosen}/>);
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).not.toHaveBeenCalled(); expect(chosen).toHaveBeenCalledOnce();
  fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
  expect(close).toHaveBeenCalledOnce();
});

it('explicit profiles use exact native identifiers, while a different model without a profile keeps native defaults', async () => {
  const choose = vi.fn(async () => true);
  const state: ModelPickerState = { choices: [{ id: 'native/current', profiles: [] }, { id: 'native/next', profiles: [{ id: 'native-profile', label: 'native-profile' }], defaultProfile: 'native-profile' }], current: 'native/current', disabled: false, loading: false, choose };
  render(<ModelPicker state={state} close={() => {}} chosen={() => {}}/>);
  await act(async () => fireEvent.click(screen.getByRole('option', { name: 'native/next / native-profile' })));
  expect(choose).toHaveBeenLastCalledWith({ kind: 'profile', model: 'native/next', profile: 'native-profile' });
  await act(async () => fireEvent.click(screen.getByRole('option', { name: 'native/next' })));
  expect(choose).toHaveBeenLastCalledWith({ kind: 'model', model: 'native/next' });
});

const profiled = [{ id: 'example/chat', profiles: [{ id: 'balanced', label: 'balanced' }, { id: 'fast', label: 'fast' }], defaultProfile: 'balanced' }];
it('a pinned Profile is the configured row; the Model row keeps it and the default row clears it, by keyboard', async () => {
  const choose = vi.fn(async () => true), chosen = vi.fn();
  render(<ModelPicker state={{ choices: profiled, current: 'example/chat', profile: 'fast', effectiveProfile: 'fast', disabled: false, loading: false, choose }} close={() => {}} chosen={chosen}/>);
  expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['example/chat', 'example/chat / Model default profile (balanced)', 'example/chat / balanced', 'example/chat / fast']);
  expect(screen.getByRole('option', { selected: true }).getAttribute('aria-label')).toBe('example/chat / fast');
  // Enter on the configured row is a genuine no-op.
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).not.toHaveBeenCalled(); expect(chosen).toHaveBeenCalledTimes(1);
  // The Model row does not clear the pinned Profile.
  for (let i = 0; i < 3; i++) fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowUp' });
  expect(screen.getByRole('option', { selected: true }).getAttribute('aria-label')).toBe('example/chat');
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).not.toHaveBeenCalled(); expect(chosen).toHaveBeenCalledTimes(2);
  // Returning to the Model default is its own explicit row.
  fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowDown' });
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'model-default', model: 'example/chat' });
  expect(chosen).toHaveBeenCalledTimes(3);
});
it('following the default, pinning the current default Profile is a change and choosing the default again is not, by pointer', async () => {
  const choose = vi.fn(async () => true), chosen = vi.fn();
  render(<ModelPicker state={{ choices: profiled, current: 'example/chat', effectiveProfile: 'balanced', disabled: false, loading: false, choose }} close={() => {}} chosen={chosen}/>);
  expect(screen.getByRole('option', { selected: true }).getAttribute('aria-label')).toBe('example/chat / Model default profile (balanced)');
  await act(async () => fireEvent.click(screen.getByRole('option', { name: 'example/chat / Model default profile (balanced)' })));
  expect(choose).not.toHaveBeenCalled(); expect(chosen).toHaveBeenCalledTimes(1);
  await act(async () => fireEvent.click(screen.getByRole('option', { name: 'example/chat / balanced' })));
  expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'profile', model: 'example/chat', profile: 'balanced' });
  expect(chosen).toHaveBeenCalledTimes(2);
  // Search reaches the default row in either locale's wording.
  act(() => localeController.setLocale('zh'));
  fireEvent.change(screen.getByRole('combobox'), { target: { value: '默认' } });
  expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['example/chat / 模型默认预设（balanced）']);
});
it('a refused selection keeps the picker open and reports nothing as chosen', async () => {
  const choose = vi.fn(async () => false), chosen = vi.fn();
  render(<ModelPicker state={{ choices: profiled, current: 'example/chat', disabled: false, loading: false, choose }} close={() => {}} chosen={chosen}/>);
  await act(async () => fireEvent.click(screen.getByRole('option', { name: 'example/chat / fast' })));
  expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'profile', model: 'example/chat', profile: 'fast' });
  expect(chosen).not.toHaveBeenCalled();
});

const unprofiled = [{ id: 'example/chat', profiles: [] }];
const staleRows = (defaults: string) => ['example/chat', `example/chat / ${defaults}`];
it('a stale pinned Profile on a Model without Profiles is cleared by its own default row, by keyboard, never by the Model row', async () => {
  const choose = vi.fn(async () => true), chosen = vi.fn();
  render(<ModelPicker state={{ choices: unprofiled, current: 'example/chat', profile: 'fast', disabled: false, loading: false, choose }} close={() => {}} chosen={chosen}/>);
  expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(staleRows('Use model defaults'));
  // The stale pin has no row: the configured Model row is active, and the
  // unavailable Profile stays identified.
  expect(screen.getByRole('option', { selected: true }).getAttribute('aria-label')).toBe('example/chat');
  expect(screen.getByRole('status').textContent).toBe('Profile fast is unavailable for example/chat in this Workspace.');
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).not.toHaveBeenCalled(); expect(chosen).toHaveBeenCalledTimes(1);
  fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowDown' });
  expect(screen.getByRole('option', { selected: true }).getAttribute('aria-label')).toBe('example/chat / Use model defaults');
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'model-default', model: 'example/chat' });
  expect(chosen).toHaveBeenCalledTimes(2);
});
it('the stale-Profile recovery row is searchable and pointer-selectable in either locale, and absent once nothing is pinned', async () => {
  const choose = vi.fn(async () => true), chosen = vi.fn();
  const state: ModelPickerState = { choices: unprofiled, current: 'example/chat', profile: 'fast', disabled: false, loading: false, choose };
  const view = render(<ModelPicker state={state} close={() => {}} chosen={chosen}/>);
  for (const [locale, defaults, query] of [['zh', '使用模型默认设置', '默认设置'], ['en', 'Use model defaults', 'use MODEL']] as const) {
    act(() => localeController.setLocale(locale));
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '' } });
    expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(staleRows(defaults));
    fireEvent.change(screen.getByRole('combobox'), { target: { value: query } });
    expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual([`example/chat / ${defaults}`]);
  }
  await act(async () => fireEvent.click(screen.getByRole('option', { name: 'example/chat / Use model defaults' })));
  expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'model-default', model: 'example/chat' });
  expect(chosen).toHaveBeenCalledOnce();
  // Once native confirms the cleared Profile, a Model without Profiles has
  // nothing to recover and offers no Profile row.
  view.rerender(<ModelPicker state={{ ...state, profile: undefined }} close={() => {}} chosen={chosen}/>);
  fireEvent.change(screen.getByRole('combobox'), { target: { value: '' } });
  expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['example/chat']);
  expect(screen.queryByRole('status')).toBeNull();
});
it('a stale pinned Profile on a Model with other Profiles is cleared by the Model default row; other Models offer no recovery row', async () => {
  const choose = vi.fn(async () => true);
  render(<ModelPicker state={{ choices: [{ id: 'example/other', profiles: [] }, ...profiled], current: 'example/chat', profile: 'retired', disabled: false, loading: false, choose }} close={() => {}} chosen={() => {}}/>);
  expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['example/other', 'example/chat', 'example/chat / Model default profile (balanced)', 'example/chat / balanced', 'example/chat / fast']);
  // The stale pin has no row; the configured Model's row is active, not the first row.
  expect(screen.getByRole('option', { selected: true }).getAttribute('aria-label')).toBe('example/chat');
  await act(async () => fireEvent.click(screen.getByRole('option', { name: 'example/chat / Model default profile (balanced)' })));
  expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'model-default', model: 'example/chat' });
});
