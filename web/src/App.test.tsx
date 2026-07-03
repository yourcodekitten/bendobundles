import { render, screen } from '@testing-library/react'
import App from './App'

describe('App', () => {
  it('renders the landing headline', () => {
    render(<App />)
    expect(screen.getByRole('heading', { name: /bendobundles/i })).toBeInTheDocument()
  })
})
