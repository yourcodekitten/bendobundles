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

  it('mature filter alone counts as an active filter (#71 review r1)', () => {
    renderBar({ mature: 'only' });
    // active-filter readout + clear button must render when only mature is set
    expect(screen.getByRole('button', { name: /clear filters/i })).toBeInTheDocument();
  });

  it('mature select renders all/hide/only and fires onChange (#71)', () => {
    const { onChange, state } = renderBar();
    const mature = screen.getByLabelText('mature') as HTMLSelectElement;
    expect([...mature.options].map((o) => o.value)).toEqual(['all', 'hide', 'only']);
    fireEvent.change(mature, { target: { value: 'hide' } });
    expect(onChange).toHaveBeenCalledWith({ ...state, mature: 'hide' });
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

describe('pick-cost control', () => {
  // 🩹 Written in THIS FILE'S idiom, not the plan's first draft. That draft used
  // `userEvent.selectOptions` and a bare `render(<ToolkitBar …/>)` — but this file
  // imports `fireEvent`, never `userEvent`, and every other control test goes
  // through `renderBar`. The draft would not have resolved `userEvent` at all.
  it('offers a pick-cost filter and reports the choice upward', () => {
    const { onChange, state } = renderBar();
    fireEvent.change(screen.getByLabelText('pick cost'), { target: { value: 'free' } });
    expect(onChange).toHaveBeenCalledWith({ ...state, cost: 'free' });
  });

  it('labels the costly option in ben-facing words, lowercase', () => {
    renderBar();
    const cost = screen.getByLabelText('pick cost') as HTMLSelectElement;
    expect([...cost.options].map((o) => o.value)).toEqual(['all', 'free', 'spends-pick']);
    expect(screen.getByRole('option', { name: 'spends a pick' })).toBeInTheDocument();
  });
});

