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
    <div className="min-h-screen bg-zinc-950 flex items-center justify-center">
      <form onSubmit={(e) => void handleSubmit(e)} className="flex flex-col gap-4 w-72">
        <h1 className="text-zinc-100 text-xl font-semibold">admin</h1>
        <label className="text-zinc-400 text-sm" htmlFor="password">
          password
        </label>
        <input
          id="password"
          name="password"
          type="password"
          autoFocus
          required
          className="bg-zinc-900 border border-zinc-700 rounded px-3 py-2 text-zinc-100 focus:outline-none focus:border-zinc-500"
        />
        <button
          type="submit"
          className="bg-zinc-800 hover:bg-zinc-700 text-zinc-100 rounded px-4 py-2"
        >
          sign in
        </button>
        {error && (
          <p role="alert" className="text-red-400 text-sm">
            nope.
          </p>
        )}
      </form>
    </div>
  );
}
