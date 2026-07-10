import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { ToolkitBar } from './ToolkitBar';
import { IDLE_TOOLKIT, type ToolkitState } from './catalogToolkit';

const tagOptions = [
  { tag: 'Action', count: 12 },
  { tag: 'Co-op', count: 4 },
];

function renderBar(over: Partial<ToolkitState> = {}, extra: Partial<Parameters<typeof ToolkitBar>[0]> = {}) {
  const state = { ...IDLE_TOOLKIT, ...over };
  const onChange = vi.fn();
  render(
    <ToolkitBar
      state={state}
      tagOptions={tagOptions}
      shown={extra.shown ?? 1081}
      total={extra.total ?? 1081}
      excludedNoData={extra.excludedNoData ?? 0}
      onChange={onChange}
    />,
  );
  return { onChange, state };
}

describe('ToolkitBar', () => {
  it('renders rating, sort, and group selects with the full option sets', () => {
    renderBar();
    const rating = screen.getByLabelText('rating') as HTMLSelectElement;
    expect([...rating.options].map((o) => o.value)).toEqual([
      'any',
      'mixed',
      'mostly-positive',
      'very-positive',
      'overwhelmingly-positive',
    ]);
    const sort = screen.getByLabelText('sort') as HTMLSelectElement;
    expect([...sort.options].map((o) => o.value)).toEqual([
      'title',
      'rating',
      'date-new',
      'date-old',
    ]);
    const group = screen.getByLabelText('group') as HTMLSelectElement;
    expect([...group.options].map((o) => o.value)).toEqual([
      'none',
      'publisher',
      'studio',
      'bundle',
    ]);
  });

  it('renders tag chips with counts inside the tags disclosure and toggles on click', () => {
    const { onChange, state } = renderBar();
    fireEvent.click(screen.getByText('tags'));
    const chip = screen.getByRole('button', { name: 'Action (12)' });
    fireEvent.click(chip);
    expect(onChange).toHaveBeenCalledWith({ ...state, tags: ['Action'] });
  });

  it('clicking a selected chip removes it', () => {
    const { onChange, state } = renderBar({ tags: ['Action', 'Co-op'] });
    fireEvent.click(screen.getByRole('button', { name: 'Action (12)' }));
    expect(onChange).toHaveBeenCalledWith({ ...state, tags: ['Co-op'] });
  });

  it('changing the rating select fires onChange with the new floor', () => {
    const { onChange, state } = renderBar();
    fireEvent.change(screen.getByLabelText('rating'), {
      target: { value: 'very-positive' },
    });
    expect(onChange).toHaveBeenCalledWith({ ...state, rating: 'very-positive' });
  });

  it('active filters: shows counts + hidden-data note + clear that keeps view prefs', () => {
    const { onChange, state } = renderBar(
      { tags: ['Action'], sort: 'rating', group: 'publisher' },
      { shown: 143, excludedNoData: 212 },
    );
    expect(screen.getByText(/showing 143 of 1081/)).toBeInTheDocument();
    expect(screen.getByText(/212 missing tag or rating data hidden/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'clear filters' }));
    expect(onChange).toHaveBeenCalledWith({
      ...IDLE_TOOLKIT,
      sort: state.sort,
      group: state.group,
    });
  });

  it('idle: plain count, no clear button', () => {
    renderBar();
    expect(screen.getByText('1081 games')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'clear filters' })).toBeNull();
  });
});
