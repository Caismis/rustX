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
  expect(screen.getAllByRole('option')).toHaveLength(2);
  expect(screen.getByText('DeepSeek')).toBeTruthy();
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'qWEN' } });
  for (const locale of ['zh', 'en'] as const) {
    act(() => localeController.setLocale(locale));
    expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['vLLM/Qwen3.6']);
    expect(choose).not.toHaveBeenCalled();
  }
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).toHaveBeenCalledExactlyOnceWith('vLLM/Qwen3.6');
  expect(chosen).toHaveBeenCalledOnce();
});
it('choosing the current model preserves its reasoning profile without another mutation', async () => {
  const choose = vi.fn(async () => true), chosen = vi.fn(), close = vi.fn();
  render(<ModelPicker state={{ choices, current: 'DeepSeek/flash', disabled: false, loading: false, choose }} close={close} chosen={chosen}/>);
  await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
  expect(choose).not.toHaveBeenCalled(); expect(chosen).toHaveBeenCalledOnce();
  fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
  expect(close).toHaveBeenCalledOnce();
});
