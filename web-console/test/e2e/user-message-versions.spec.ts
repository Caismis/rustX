import { expect, test, type Locator } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { EditorPage, UserInputBlock } from '../../../protocol/app-server/v82';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';
import { routeWorkspaceHost } from './workspace-host';

const original = {
  prompt: 'Draft a harbor summary.',
  answer: 'The harbor version is complete.',
};
const edited = {
  prompt: 'Draft an orchard summary.\nKeep the heading "Café".',
  answer: 'The orchard version is complete.',
};
const regenerated = { ...edited, answer: 'The regenerated orchard version is complete.' };
const attachment = { name: 'selected-input.txt', text: 'Native input retained across message versions.' };

type ProviderRequest = {
  body: { messages: { role: string; content: string | { type: string; text?: string }[] }[] };
};

function userPrompts(request: ProviderRequest) {
  return request.body.messages.filter(message => message.role === 'user').map(({ content }) =>
    typeof content === 'string' ? content : content.filter(part => part.type === 'text').map(part => part.text).join(''),
  ).filter(text => !(text.startsWith('<system-reminder>\n') && text.endsWith('\n</system-reminder>')));
}

test('edits and retries a user message, then pages through durable conversation versions', async ({ page }) => {
  const fixture = await startDogfood('web_user_message_versions');
  let passed = false;
  try {
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    const sessionId = await openEmptySession(page, fixture, 'Workspace A');
    const session = page.locator(`button[data-session-id="${sessionId}"]`);
    const chat = page.getByLabel('Canonical conversation', { exact: true });
    const userMessage = chat.getByRole('article', { name: 'Your message', exact: true });
    const editor = userMessage.getByRole('textbox', { name: 'Edit your message', exact: true });
    const previousVersion = (row: Locator) => row.getByRole('button', { name: 'Switch to previous message version', exact: true });
    const nextVersion = (row: Locator) => row.getByRole('button', { name: 'Switch to next message version', exact: true });
    const previous = previousVersion(userMessage);
    const next = nextVersion(userMessage);
    const chatTab = page.getByRole('tab', { name: 'Chat', exact: true });
    const trajectoryTab = page.getByRole('tab', { name: 'Trajectory', exact: true });
    const ledger = page.getByRole('table', { name: 'Trace ledger', exact: true });
    const traceUserMessage = ledger.locator('[data-kind="user"]');
    const inspector = page.getByRole('complementary', { name: 'Event details', exact: true });
    const answer = (text: string) => chat.getByText(text, { exact: true });
    const attachedFile = userMessage.getByText(attachment.name, { exact: true });
    const providerRequests = async (): Promise<ProviderRequest[]> => (await fixture.control('requests')).requests;
    const submissions = (): UserInputBlock[][] => wire.requests.filter(request => request.method === 'inbound/queue')
      .map(request => request.params.content);
    const editorReceipts = () => wire.responses.filter(response => response.method === 'session/editorRead')
      .flatMap(({ result }: { result?: { page: EditorPage } }) => result?.page.upload ? [result.page.upload] : []);
    const expectInference = async (count: number) => {
      await expect.poll(async () => (await providerRequests()).length, { timeout: 20_000 }).toBe(count);
    };
    const expectPager = async (row: Locator, index: number, total: number) => {
      await expect(row.getByRole('status').filter({ hasText: new RegExp(`^${index}/${total}$`) })).toBeVisible();
      for (const [control, enabled] of [[previousVersion(row), index > 1], [nextVersion(row), index < total]] as const) {
        if (enabled) await expect(control).toBeEnabled();
        else await expect(control).toBeDisabled();
      }
    };
    const expectVersion = async ({ prompt, answer: text }: typeof original, index: number, total: number) => {
      await expect(session).toHaveAttribute('aria-current', 'page');
      await expect(userMessage).toHaveCount(1);
      await expect(userMessage.getByText(prompt, { exact: true })).toBeVisible();
      await expect(attachedFile).toBeVisible();
      await expect(answer(text)).toBeVisible();
      await expectPager(userMessage, index, total);
    };
    const expectTraceVersion = async ({ prompt, answer: text }: typeof original, index: number, total: number) => {
      await expect(trajectoryTab).toHaveAttribute('aria-selected', 'true');
      await expectPager(traceUserMessage, index, total);
      await traceUserMessage.click();
      await expect(inspector).toContainText(prompt.split('\n')[0]);
      const assistant = ledger.locator('[data-kind="assistant"]');
      await expect(assistant).toContainText(text);
      await assistant.click();
      await expect(inspector).toContainText(text);
    };

    await page.getByRole('textbox', { name: 'Message', exact: true }).fill(original.prompt);
    await page.getByLabel('Attach files').setInputFiles({
      name: attachment.name, mimeType: 'text/plain', buffer: Buffer.from(attachment.text),
    });
    await expect(page.locator('[data-attachment-state=ready]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expectInference(1);
    const upload = submissions()[0]?.find(block => block.type === 'upload');
    if (!upload) throw new Error('The original submission did not contain its native upload receipt.');
    const { type: _, ...receipt } = upload;
    expect(receipt).toEqual({ session_id: sessionId, batch_id: expect.any(String), token: expect.any(String) });
    const attachmentPath = join(fixture.workspaceA, '.agents/uploads', sessionId, receipt.batch_id, attachment.name);
    expect(readFileSync(attachmentPath, 'utf8')).toBe(attachment.text);
    const fileContext = `<user_uploaded_files>\n  <file name="${attachment.name}" path="${attachmentPath}" />\n</user_uploaded_files>\n\n`;
    const expectPrompts = async (...prompts: string[]) => {
      expect(submissions()).toEqual(prompts.map(text => [upload, { type: 'text', text }]));
      expect((await providerRequests()).map(userPrompts)).toEqual(prompts.map(prompt => [fileContext + prompt]));
    };
    await expectPrompts(original.prompt);
    await expect(answer(original.answer)).toBeVisible();
    await expect(attachedFile).toBeVisible();
    await expect(previous).toHaveCount(0);
    await expect(next).toHaveCount(0);
    await trajectoryTab.click();
    await expect(traceUserMessage).toBeVisible();
    await expect(previousVersion(traceUserMessage)).toHaveCount(0);
    await expect(nextVersion(traceUserMessage)).toHaveCount(0);
    await chatTab.click();

    await userMessage.getByRole('button', { name: 'Edit message', exact: true }).click();
    await expect(editor).toHaveValue(original.prompt);
    await editor.fill(edited.prompt);
    wire.holdNext('session/branch');
    try {
      await userMessage.getByRole('button', { name: 'Retry', exact: true }).click();
      await expect.poll(wire.held).toBe(1);
      expect(wire.requests.filter(request => request.method === 'session/branch').map(request => request.params))
        .toEqual([expect.objectContaining({ session_id: sessionId, side: 'before' })]);
      await expect(userMessage.getByText(edited.prompt, { exact: true })).toBeVisible();
      await expect(userMessage.getByText(original.prompt, { exact: true })).toBeHidden();
      await expect(answer(original.answer)).toBeHidden();
      await expect(attachedFile).toBeVisible();
      await expectPrompts(original.prompt);
    } finally {
      wire.releaseHeld('session/branch');
    }
    await expectInference(2);
    await expectPrompts(original.prompt, edited.prompt);
    expect(editorReceipts()).toEqual([receipt]);
    await expectVersion(edited, 2, 2);
    await expect(answer(original.answer)).toBeHidden();
    await trajectoryTab.click();
    await expectTraceVersion(edited, 2, 2);
    await chatTab.click();

    await previous.click();
    await expectVersion(original, 1, 2);
    await expect(answer(edited.answer)).toBeHidden();
    await trajectoryTab.click();
    await expectTraceVersion(original, 1, 2);
    await chatTab.click();
    await expectPrompts(original.prompt, edited.prompt);

    await page.reload();
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expectVersion(original, 1, 2);
    await expectPrompts(original.prompt, edited.prompt);

    await next.click();
    await expectVersion(edited, 2, 2);
    await expect(answer(original.answer)).toBeHidden();
    await expectPrompts(original.prompt, edited.prompt);

    await chat.getByRole('button', { name: 'Regenerate', exact: true }).click();
    await expectInference(3);
    await expectPrompts(original.prompt, edited.prompt, edited.prompt);
    expect(editorReceipts()).toEqual([receipt, receipt]);
    await expectVersion(regenerated, 3, 3);
    await trajectoryTab.click();
    await expectTraceVersion(regenerated, 3, 3);
    await previousVersion(traceUserMessage).click();
    await expectTraceVersion(edited, 2, 3);
    await chatTab.click();
    await expectVersion(edited, 2, 3);
    await expect(answer(regenerated.answer)).toBeHidden();

    await trajectoryTab.click();
    await previousVersion(traceUserMessage).click();
    await expectTraceVersion(original, 1, 3);
    await nextVersion(traceUserMessage).click();
    await expectTraceVersion(edited, 2, 3);
    await nextVersion(traceUserMessage).click();
    await expectTraceVersion(regenerated, 3, 3);
    await chatTab.click();
    await expectVersion(regenerated, 3, 3);
    await expectPrompts(original.prompt, edited.prompt, edited.prompt);
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});
