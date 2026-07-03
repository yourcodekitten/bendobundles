import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { Login } from './Login';

vi.mock('../api');
import { adminLogin } from '../api';

function renderLogin() {
  return render(
    <MemoryRouter initialEntries={['/admin/login']}>
      <Routes>
        <Route path="/admin/login" element={<Login />} />
        <Route path="/admin/catalog" element={<div>catalog page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('Login', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('has password input with autoFocus and label', () => {
    renderLogin();
    const input = screen.getByLabelText(/password/i);
    expect(input).toHaveAttribute('type', 'password');
    expect(input).toHaveFocus();
  });

  it('does not show error message initially', () => {
    renderLogin();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('success navigates to /admin/catalog', async () => {
    const user = userEvent.setup();
    vi.mocked(adminLogin).mockResolvedValue(true);
    renderLogin();

    await user.type(screen.getByLabelText(/password/i), 'hunter2');
    await user.click(screen.getByRole('button', { name: /sign in/i }));

    await waitFor(() => {
      expect(screen.getByText('catalog page')).toBeInTheDocument();
    });
  });

  it('failure shows "nope." inline and does not navigate', async () => {
    const user = userEvent.setup();
    vi.mocked(adminLogin).mockResolvedValue(false);
    renderLogin();

    await user.type(screen.getByLabelText(/password/i), 'wrongpassword');
    await user.click(screen.getByRole('button', { name: /sign in/i }));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent('nope.');
    });
    // still on login page
    expect(screen.queryByText('catalog page')).not.toBeInTheDocument();
  });

  it('clears password field after submit — value absent from DOM', async () => {
    const user = userEvent.setup();
    vi.mocked(adminLogin).mockResolvedValue(false);
    renderLogin();

    const input = screen.getByLabelText(/password/i) as HTMLInputElement;
    await user.type(input, 'secretpassword');
    expect(input.value).toBe('secretpassword');

    await user.click(screen.getByRole('button', { name: /sign in/i }));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });

    // field must be cleared
    expect(input.value).toBe('');
    // value must not appear anywhere in the DOM
    expect(document.body.innerHTML).not.toContain('secretpassword');
  });
});
