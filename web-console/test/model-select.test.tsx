import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ModelSelect } from '../src/presentation/agent/ModelSelect';
import { localeController } from '../src/locale/controller';

afterEach(() => { cleanup(); act(() => localeController.setLocale('en')); });
const choices = [{ id: 'native/chat', profiles: [{ id: 'high', label: 'High' }, { id: 'low', label: 'Low' }], defaultProfile: 'high' }];
const props = { choices, current: 'native/chat', disabled: false, loading: false, load: vi.fn(), choose: vi.fn(async () => true) };
const trigger = () => screen.getByRole('button', { name: 'Model and thinking effort' });

it('uses one card for model and effort panes, one named default, and DSH keyboard return focus', async () => {
 render(<ModelSelect {...props}/>);
 fireEvent.click(trigger());
 const card = screen.getByRole('menu');
 expect(screen.getAllByRole('menuitem').map(row => row.getAttribute('aria-label'))).toEqual(['Model', 'Thinking effort']);
 expect(card.textContent).toBe('Modelnative/chatThinking effortHigh');
 fireEvent.click(screen.getByRole('menuitem', { name: 'Thinking effort' }));
 expect(screen.getByRole('menu')).toBe(card);
 expect(screen.getAllByRole('menuitemradio').map(row => row.textContent)).toEqual(['High', 'Low']);
 expect(screen.getByRole('menuitemradio', { name: 'High', checked: true })).toBe(document.activeElement);
 fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
 expect(screen.getByRole('menu')).toBe(card);
 expect(screen.getByRole('menuitem', { name: 'Thinking effort' })).toBe(document.activeElement);
 fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
 expect(screen.queryByRole('menu')).toBeNull(); expect(document.activeElement).toBe(trigger());
});

it('retains the card and confirmed label throughout selection and closes only after confirmation', async () => {
 let finish!: (ok: boolean) => void;
 const choose = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; }));
 const ui = render(<ModelSelect {...props} choose={choose}/>);
 fireEvent.click(trigger()); fireEvent.click(screen.getByRole('menuitem', { name: 'Thinking effort' }));
 const card = screen.getByRole('menu'), row = screen.getByRole('menuitemradio', { name: 'Low' });
 fireEvent.click(row);
 expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'profile', model: 'native/chat', profile: 'low' });
 expect(trigger().textContent).toBe('native/chatHigh'); expect(trigger().getAttribute('aria-busy')).toBe('true');
 expect(screen.getByRole('menu')).toBe(card); expect(screen.getByRole('menuitemradio', { name: 'Low' })).toBe(row);
 expect(row).toHaveProperty('disabled', true); expect(row.querySelector('[data-state="ongoing"]')).not.toBeNull();
 expect(document.activeElement).toBe(trigger());
 ui.rerender(<ModelSelect {...props} choose={choose} disabled loading/>);
 expect(trigger()).toHaveProperty('disabled', false); expect(document.activeElement).toBe(trigger());
 expect(screen.getByRole('menu')).toBe(card);
 ui.rerender(<ModelSelect {...props} choose={choose} profile="low" effectiveProfile="low"/>);
 expect(screen.getByRole('menu')).toBe(card);
 await act(async () => finish(true));
 expect(screen.queryByRole('menu')).toBeNull(); expect(trigger().textContent).toBe('native/chatLow');
 expect(trigger().getAttribute('data-selection-focus')).toBe(''); expect(document.activeElement).toBe(trigger());
});

it('searches larger native catalogs with DSH ranking and keyboard selection while retaining search focus', async () => {
 const choose = vi.fn(async () => true);
 render(<ModelSelect {...props} choices={[...choices, ...['beta', 'gamma', 'delta', 'chat-plus'].map(id => ({ id: `native/${id}`, profiles: [] }))]} choose={choose}/>);
 fireEvent.click(trigger()); fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 const search = screen.getByRole('searchbox'); expect(document.activeElement).toBe(search);
 fireEvent.change(search, { target: { value: 'cp' } });
 expect(screen.getAllByRole('menuitemradio').map(row => row.getAttribute('aria-label'))).toEqual(['native/chat-plus']);
 fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
 expect(search).toHaveProperty('value', ''); expect(document.activeElement).toBe(search);
 fireEvent.change(search, { target: { value: 'GaMmA' } });
 fireEvent.keyDown(search, { key: 'ArrowDown' }); expect(document.activeElement).toBe(search);
 await act(async () => fireEvent.keyDown(search, { key: 'Tab' }));
 expect(choose).toHaveBeenCalledExactlyOnceWith({ kind: 'model', model: 'native/gamma' });
 expect(document.activeElement).toBe(trigger());
});

it('a replaced attachment cannot be closed or kept busy by its previous selection', async () => {
 let finish!: (ok: boolean) => void;
 const choose = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve; }));
 const ui = render(<ModelSelect {...props} choose={choose} binding="A"/>);
 fireEvent.click(trigger()); fireEvent.click(screen.getByRole('menuitem', { name: 'Thinking effort' })); fireEvent.click(screen.getByRole('menuitemradio', { name: 'Low' }));
 ui.rerender(<ModelSelect {...props} choose={choose} binding="B"/>);
 fireEvent.click(trigger()); const card = screen.getByRole('menu');
 await act(async () => finish(true));
 expect(screen.getByRole('menu')).toBe(card); expect(trigger().getAttribute('aria-busy')).toBe('false');
});

it('a native strength named default stays distinct from an unnamed provider default', () => {
 const errors = vi.spyOn(console, 'error');
 render(<ModelSelect {...props} choices={[{ id: 'native/chat', profiles: [{ id: 'default', label: 'Native default strength' }] }]}/>);
 fireEvent.click(trigger()); fireEvent.click(screen.getByRole('menuitem', { name: 'Thinking effort' }));
 expect(screen.getAllByRole('menuitemradio').map(row => row.textContent)).toEqual(['Default', 'Native default strength']);
 expect(screen.getByRole('menuitemradio', { name: 'Default' }).getAttribute('aria-checked')).toBe('true');
 expect(errors).not.toHaveBeenCalled();
});
