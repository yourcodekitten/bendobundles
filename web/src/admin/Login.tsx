import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { adminLogin } from '../api';

export function Login() {
  const navigate = useNavigate();
  const [error, setError] = useState(false);

  async function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const input = form.elements.namedItem('password') as HTMLInputElement;
    const password = input.value;
    // Clear the field immediately — password value must not remain in the DOM after submit
    input.value = '';
    setError(false);

    const ok = await adminLogin(password);
    if (ok) {
      navigate('/admin/catalog');
    } else {
      setError(true);
    }
  }

  return (
    <div className="min-h-screen bg-room flex items-center justify-center">
      <form onSubmit={(e) => void handleSubmit(e)} className="flex flex-col gap-4 w-72">
        <h1 className="text-ink text-xl font-semibold">admin</h1>
        <label className="text-dust text-sm" htmlFor="password">
          password
        </label>
        <input
          id="password"
          name="password"
          type="password"
          autoFocus
          required
          className="bg-floor border border-line rounded px-3 py-2 text-ink focus:outline-none focus:border-pixel"
        />
        <button
          type="submit"
          className="bg-shelf hover:bg-control text-ink rounded px-4 py-2"
        >
          sign in
        </button>
        {error && (
          <p role="alert" className="text-red-700 text-sm">
            nope.
          </p>
        )}
      </form>
    </div>
  );
}
