//! Helper functions for initialising GridTrack's from styles
//! This mainly consists of evaluating GridAutoTracks
use super::types::{GridTrack, TrackCounts};
use crate::geometry::{AbsoluteAxis, Size};
use crate::style::{GridTrackRepetition, LengthPercentage, NonRepeatedTrackSizingFunction, TrackSizingFunction};
use crate::style_helpers::TaffyAuto;
use crate::util::sys::{ceil, floor, Vec};
use crate::util::MaybeMath;
use crate::util::ResolveOrZero;
use crate::{GridContainerStyle, MaybeResolve};

/// Compute the number of rows and columns in the explicit grid
pub(crate) fn compute_explicit_grid_size_in_axis(
    style: &impl GridContainerStyle,
    template: &[TrackSizingFunction],
    inner_container_size: Size<Option<f32>>,
    resolve_calc_value: impl Fn(u64, f32) -> f32,
    axis: AbsoluteAxis,
) -> u16 {
    // If template contains no tracks, then there are trivially zero explicit tracks
    if template.is_empty() {
        return 0;
    }

    // If there are any repetitions that contains no tracks, then the whole definition should be considered invalid
    // and we default to no explicit tracks
    let template_has_repetitions_with_zero_tracks = template.iter().any(|track_def| match track_def {
        TrackSizingFunction::Single(_) => false,
        TrackSizingFunction::Repeat(_, tracks) => tracks.is_empty(),
    });
    if template_has_repetitions_with_zero_tracks {
        return 0;
    }

    // Compute that number of track generated by single track definition and repetitions with a fixed repetition count
    let non_auto_repeating_track_count = template
        .iter()
        .map(|track_def| {
            use GridTrackRepetition::{AutoFill, AutoFit, Count};
            match track_def {
                TrackSizingFunction::Single(_) => 1,
                TrackSizingFunction::Repeat(Count(count), tracks) => count * tracks.len() as u16,
                TrackSizingFunction::Repeat(AutoFit | AutoFill, _) => 0,
            }
        })
        .sum::<u16>();

    let auto_repetition_count = template.iter().filter(|track_def| track_def.is_auto_repetition()).count() as u16;
    let all_track_defs_have_fixed_component = template.iter().all(|track_def| match track_def {
        TrackSizingFunction::Single(sizing_function) => sizing_function.has_fixed_component(),
        TrackSizingFunction::Repeat(_, tracks) => {
            tracks.iter().all(|sizing_function| sizing_function.has_fixed_component())
        }
    });

    let template_is_valid =
        auto_repetition_count == 0 || (auto_repetition_count == 1 && all_track_defs_have_fixed_component);

    // If the template is invalid because it contains multiple auto-repetition definitions or it combines an auto-repetition
    // definition with non-fixed-size track sizing functions, then disregard it entirely and default to zero explicit tracks
    if !template_is_valid {
        return 0;
    }

    // If there are no repetitions, then the number of explicit tracks is simply equal to the lengths of the track definition
    // vector (as each item in the Vec represents one track).
    if auto_repetition_count == 0 {
        return non_auto_repeating_track_count;
    }

    let repetition_definition = template
        .iter()
        .find_map(|def| {
            use GridTrackRepetition::{AutoFill, AutoFit, Count};
            match def {
                TrackSizingFunction::Single(_) => None,
                TrackSizingFunction::Repeat(Count(_), _) => None,
                TrackSizingFunction::Repeat(AutoFit | AutoFill, tracks) => Some(tracks),
            }
        })
        .unwrap();
    let repetition_track_count = repetition_definition.len() as u16;

    // Otherwise, run logic to resolve the auto-repeated track count:
    //
    // If the grid container has a definite size or max size in the relevant axis:
    //   - then the number of repetitions is the largest possible positive integer that does not cause the grid to overflow the content
    //     box of its grid container.
    // Otherwise, if the grid container has a definite min size in the relevant axis:
    //   - then the number of repetitions is the smallest possible positive integer that fulfills that minimum requirement
    // Otherwise, the specified track list repeats only once.
    let style_size_is_definite =
        style.size().get_abs(axis).maybe_resolve(inner_container_size.get_abs(axis), &resolve_calc_value).is_some();
    let style_max_size_is_definite =
        style.max_size().get_abs(axis).maybe_resolve(inner_container_size.get_abs(axis), &resolve_calc_value).is_some();
    let size_is_maximum = style_size_is_definite | style_max_size_is_definite;

    // Determine the number of repetitions
    let num_repetitions: u16 = match inner_container_size.get_abs(axis) {
        None => 1,
        Some(inner_container_size) => {
            let parent_size = Some(inner_container_size);

            /// ...treating each track as its max track sizing function if that is definite or as its minimum track sizing function
            /// otherwise, flooring the max track sizing function by the min track sizing function if both are definite
            fn track_definite_value(
                sizing_function: &NonRepeatedTrackSizingFunction,
                parent_size: Option<f32>,
                calc_resolver: impl Fn(u64, f32) -> f32,
            ) -> f32 {
                let max_size = sizing_function.max.definite_value(parent_size, &calc_resolver);
                let min_size = sizing_function.min.definite_value(parent_size, &calc_resolver);
                max_size.map(|max| max.maybe_min(min_size)).or(min_size).unwrap()
            }

            let non_repeating_track_used_space: f32 = template
                .iter()
                .map(|track_def| {
                    use GridTrackRepetition::{AutoFill, AutoFit, Count};
                    match track_def {
                        TrackSizingFunction::Single(sizing_function) => {
                            track_definite_value(sizing_function, parent_size, &resolve_calc_value)
                        }
                        TrackSizingFunction::Repeat(Count(count), repeated_tracks) => {
                            let sum = repeated_tracks
                                .iter()
                                .map(|sizing_function| {
                                    track_definite_value(sizing_function, parent_size, &resolve_calc_value)
                                })
                                .sum::<f32>();
                            sum * (*count as f32)
                        }
                        TrackSizingFunction::Repeat(AutoFit | AutoFill, _) => 0.0,
                    }
                })
                .sum();
            let gap_size = style.gap().get_abs(axis).resolve_or_zero(Some(inner_container_size), &resolve_calc_value);

            // Compute the amount of space that a single repetition of the repeated track list takes
            let per_repetition_track_used_space: f32 = repetition_definition
                .iter()
                .map(|sizing_function| track_definite_value(sizing_function, parent_size, &resolve_calc_value))
                .sum::<f32>();

            // We special case the first repetition here because the number of gaps in the first repetition
            // depends on the number of non-repeating tracks in the template
            let first_repetition_and_non_repeating_tracks_used_space = non_repeating_track_used_space
                + per_repetition_track_used_space
                + ((non_auto_repeating_track_count + repetition_track_count).saturating_sub(1) as f32 * gap_size);

            // If a single repetition already overflows the container then we return 1 as the repetition count
            // (the number of repetitions is floored at 1)
            if first_repetition_and_non_repeating_tracks_used_space > inner_container_size {
                1u16
            } else {
                let per_repetition_gap_used_space = (repetition_definition.len() as f32) * gap_size;
                let per_repetition_used_space = per_repetition_track_used_space + per_repetition_gap_used_space;
                let num_repetition_that_fit = (inner_container_size
                    - first_repetition_and_non_repeating_tracks_used_space)
                    / per_repetition_used_space;

                // If the container size is a preferred or maximum size:
                //   Then we return the maximum number of repetitions that fit into the container without overflowing.
                // If the container size is a minimum size:
                //   - Then we return the minimum number of repetitions required to overflow the size.
                //
                // In all cases we add the additional repetition that was already accounted for in the special-case computation above
                if size_is_maximum {
                    (floor(num_repetition_that_fit) as u16) + 1
                } else {
                    (ceil(num_repetition_that_fit) as u16) + 1
                }
            }
        }
    };

    non_auto_repeating_track_count + (repetition_track_count * num_repetitions)
}

/// Resolve the track sizing functions of explicit tracks, automatically created tracks, and gutters
/// given a set of track counts and all of the relevant styles
pub(super) fn initialize_grid_tracks(
    tracks: &mut Vec<GridTrack>,
    counts: TrackCounts,
    track_template: &[TrackSizingFunction],
    auto_tracks: &[NonRepeatedTrackSizingFunction],
    gap: LengthPercentage,
    track_has_items: impl Fn(usize) -> bool,
) {
    // Clear vector (in case this is a re-layout), reserve space for all tracks ahead of time to reduce allocations,
    // and push the initial gutter
    tracks.clear();
    tracks.reserve((counts.len() * 2) + 1);
    tracks.push(GridTrack::gutter(gap));

    // Create negative implicit tracks
    if counts.negative_implicit > 0 {
        if auto_tracks.is_empty() {
            let iter = core::iter::repeat(NonRepeatedTrackSizingFunction::AUTO);
            create_implicit_tracks(tracks, counts.negative_implicit, iter, gap)
        } else {
            let offset = auto_tracks.len() - (counts.negative_implicit as usize % auto_tracks.len());
            let iter = auto_tracks.iter().copied().cycle().skip(offset);
            create_implicit_tracks(tracks, counts.negative_implicit, iter, gap)
        }
    }

    let mut current_track_index = (counts.negative_implicit) as usize;

    // Create explicit tracks
    // An explicit check against the count (rather than just relying on track_template being empty) is required here
    // because a count of zero can result from the track_template being invalid, in which case it should be ignored.
    if counts.explicit > 0 {
        track_template.iter().for_each(|track_sizing_function| {
            use GridTrackRepetition::{AutoFill, AutoFit, Count};
            match track_sizing_function {
                TrackSizingFunction::Single(sizing_function) => {
                    tracks.push(GridTrack::new(
                        sizing_function.min_sizing_function(),
                        sizing_function.max_sizing_function(),
                    ));
                    tracks.push(GridTrack::gutter(gap));
                    current_track_index += 1;
                }
                TrackSizingFunction::Repeat(Count(count), repeated_tracks) => {
                    let track_iter = repeated_tracks.iter().cycle().take(repeated_tracks.len() * *count as usize);
                    track_iter.for_each(|sizing_function| {
                        tracks.push(GridTrack::new(
                            sizing_function.min_sizing_function(),
                            sizing_function.max_sizing_function(),
                        ));
                        tracks.push(GridTrack::gutter(gap));
                        current_track_index += 1;
                    });
                }
                TrackSizingFunction::Repeat(repetition_kind @ (AutoFit | AutoFill), repeated_tracks) => {
                    let auto_repeated_track_count = (counts.explicit - (track_template.len() as u16 - 1)) as usize;
                    let iter = repeated_tracks.iter().copied().cycle();
                    for track_def in iter.take(auto_repeated_track_count) {
                        let mut track =
                            GridTrack::new(track_def.min_sizing_function(), track_def.max_sizing_function());
                        let mut gutter = GridTrack::gutter(gap);

                        // Auto-fit tracks that don't contain should be collapsed.
                        if *repetition_kind == AutoFit && !track_has_items(current_track_index) {
                            track.collapse();
                            gutter.collapse();
                        }

                        tracks.push(track);
                        tracks.push(gutter);

                        current_track_index += 1;
                    }
                }
            }
        });
    }

    // Create positive implicit tracks
    if auto_tracks.is_empty() {
        let iter = core::iter::repeat(NonRepeatedTrackSizingFunction::AUTO);
        create_implicit_tracks(tracks, counts.positive_implicit, iter, gap)
    } else {
        let iter = auto_tracks.iter().copied().cycle();
        create_implicit_tracks(tracks, counts.positive_implicit, iter, gap)
    }

    // Mark first and last grid lines as collapsed
    tracks.first_mut().unwrap().collapse();
    tracks.last_mut().unwrap().collapse();
}

/// Utility function for repeating logic of creating implicit tracks
fn create_implicit_tracks(
    tracks: &mut Vec<GridTrack>,
    count: u16,
    mut auto_tracks_iter: impl Iterator<Item = NonRepeatedTrackSizingFunction>,
    gap: LengthPercentage,
) {
    for _ in 0..count {
        let track_def = auto_tracks_iter.next().unwrap();
        tracks.push(GridTrack::new(track_def.min_sizing_function(), track_def.max_sizing_function()));
        tracks.push(GridTrack::gutter(gap));
    }
}

#[cfg(test)]
mod test {
    use super::compute_explicit_grid_size_in_axis;
    use super::initialize_grid_tracks;
    use crate::compute::grid::types::GridTrackKind;
    use crate::compute::grid::types::TrackCounts;
    use crate::compute::grid::util::*;
    use crate::geometry::AbsoluteAxis;
    use crate::prelude::*;

    #[test]
    fn explicit_grid_sizing_no_repeats() {
        let grid_style = (600.0, 600.0, 2, 4).into_grid();
        let preferred_size = grid_style.size.map(|s| s.into_option());
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 2);
        assert_eq!(height, 4);
    }

    #[test]
    fn explicit_grid_sizing_auto_fill_exact_fit() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            size: Size { width: length(120.0), height: length(80.0) },
            grid_template_columns: vec![repeat(AutoFill, vec![length(40.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0)])],
            ..Default::default()
        };
        let preferred_size = grid_style.size.map(|s| s.into_option());
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 3);
        assert_eq!(height, 4);
    }

    #[test]
    fn explicit_grid_sizing_auto_fill_non_exact_fit() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            size: Size { width: length(140.0), height: length(90.0) },
            grid_template_columns: vec![repeat(AutoFill, vec![length(40.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0)])],
            ..Default::default()
        };
        let preferred_size = grid_style.size.map(|s| s.into_option());
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 3);
        assert_eq!(height, 4);
    }

    #[test]
    fn explicit_grid_sizing_auto_fill_min_size_exact_fit() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            min_size: Size { width: length(120.0), height: length(80.0) },
            grid_template_columns: vec![repeat(AutoFill, vec![length(40.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0)])],
            ..Default::default()
        };
        let inner_container_size = Size { width: Some(120.0), height: Some(80.0) };
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            inner_container_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            inner_container_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 3);
        assert_eq!(height, 4);
    }

    #[test]
    fn explicit_grid_sizing_auto_fill_min_size_non_exact_fit() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            min_size: Size { width: length(140.0), height: length(90.0) },
            grid_template_columns: vec![repeat(AutoFill, vec![length(40.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0)])],
            ..Default::default()
        };
        let inner_container_size = Size { width: Some(140.0), height: Some(90.0) };
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            inner_container_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            inner_container_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 4);
        assert_eq!(height, 5);
    }

    #[test]
    fn explicit_grid_sizing_auto_fill_multiple_repeated_tracks() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            size: Size { width: length(140.0), height: length(100.0) },
            grid_template_columns: vec![repeat(AutoFill, vec![length(40.0), length(20.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0), length(10.0)])],
            ..Default::default()
        };
        let preferred_size = grid_style.size.map(|s| s.into_option());
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 4); // 2 repetitions * 2 repeated tracks = 4 tracks in total
        assert_eq!(height, 6); // 3 repetitions * 2 repeated tracks = 4 tracks in total
    }

    #[test]
    fn explicit_grid_sizing_auto_fill_gap() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            size: Size { width: length(140.0), height: length(100.0) },
            grid_template_columns: vec![repeat(AutoFill, vec![length(40.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0)])],
            gap: length(20.0),
            ..Default::default()
        };
        let preferred_size = grid_style.size.map(|s| s.into_option());
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 2); // 2 tracks + 1 gap
        assert_eq!(height, 3); // 3 tracks + 2 gaps
    }

    #[test]
    fn explicit_grid_sizing_no_defined_size() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            grid_template_columns: vec![repeat(AutoFill, vec![length(40.0), percent(0.5), length(20.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0)])],
            gap: length(20.0),
            ..Default::default()
        };
        let preferred_size = grid_style.size.map(|s| s.into_option());
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 3);
        assert_eq!(height, 1);
    }

    #[test]
    fn explicit_grid_sizing_mix_repeated_and_non_repeated() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            size: Size { width: length(140.0), height: length(100.0) },
            grid_template_columns: vec![length(20.0), repeat(AutoFill, vec![length(40.0)])],
            grid_template_rows: vec![length(40.0), repeat(AutoFill, vec![length(20.0)])],
            gap: length(20.0),
            ..Default::default()
        };
        let preferred_size = grid_style.size.map(|s| s.into_option());
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            preferred_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 3); // 3 tracks + 2 gaps
        assert_eq!(height, 2); // 2 tracks + 1 gap
    }

    #[test]
    fn explicit_grid_sizing_mix_with_padding() {
        use GridTrackRepetition::AutoFill;
        let grid_style = Style {
            display: Display::Grid,
            size: Size { width: length(120.0), height: length(120.0) },
            padding: Rect { left: length(10.0), right: length(10.0), top: length(20.0), bottom: length(20.0) },
            grid_template_columns: vec![repeat(AutoFill, vec![length(20.0)])],
            grid_template_rows: vec![repeat(AutoFill, vec![length(20.0)])],
            ..Default::default()
        };
        let inner_container_size = Size { width: Some(100.0), height: Some(80.0) };
        let width = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_columns,
            inner_container_size,
            |_, _| 42.42,
            AbsoluteAxis::Horizontal,
        );
        let height = compute_explicit_grid_size_in_axis(
            &grid_style,
            &grid_style.grid_template_rows,
            inner_container_size,
            |_, _| 42.42,
            AbsoluteAxis::Vertical,
        );
        assert_eq!(width, 5); // 40px horizontal padding
        assert_eq!(height, 4); // 20px vertical padding
    }

    #[test]
    fn test_initialize_grid_tracks() {
        let minpx0 = MinTrackSizingFunction::from_length(0.0);
        let minpx20 = MinTrackSizingFunction::from_length(20.0);
        let minpx100 = MinTrackSizingFunction::from_length(100.0);

        let maxpx0 = MaxTrackSizingFunction::from_length(0.0);
        let maxpx20 = MaxTrackSizingFunction::from_length(20.0);
        let maxpx100 = MaxTrackSizingFunction::from_length(100.0);

        // Setup test
        let track_template = vec![length(100.0), minmax(length(100.0), fr(2.0)), fr(1.0)];
        let track_counts =
            TrackCounts { negative_implicit: 3, explicit: track_template.len() as u16, positive_implicit: 3 };
        let auto_tracks = vec![auto(), length(100.0)];
        let gap = LengthPercentage::from_length(20.0);

        // Call function
        let mut tracks = Vec::new();
        initialize_grid_tracks(&mut tracks, track_counts, &track_template, &auto_tracks, gap, |_| false);

        // Assertions
        let expected = vec![
            // Gutter
            (GridTrackKind::Gutter, minpx0, maxpx0),
            // Negative implicit tracks
            (GridTrackKind::Track, minpx100, maxpx100),
            (GridTrackKind::Gutter, minpx20, maxpx20),
            (GridTrackKind::Track, auto(), auto()),
            (GridTrackKind::Gutter, minpx20, maxpx20),
            (GridTrackKind::Track, minpx100, maxpx100),
            (GridTrackKind::Gutter, minpx20, maxpx20),
            // Explicit tracks
            (GridTrackKind::Track, minpx100, maxpx100),
            (GridTrackKind::Gutter, minpx20, maxpx20),
            (GridTrackKind::Track, minpx100, MaxTrackSizingFunction::from_fr(2.0)), // Note: separate min-max functions
            (GridTrackKind::Gutter, minpx20, maxpx20),
            (GridTrackKind::Track, auto(), MaxTrackSizingFunction::from_fr(1.0)), // Note: min sizing function of flex sizing functions is AUTO
            (GridTrackKind::Gutter, minpx20, maxpx20),
            // Positive implicit tracks
            (GridTrackKind::Track, auto(), auto()),
            (GridTrackKind::Gutter, minpx20, maxpx20),
            (GridTrackKind::Track, minpx100, maxpx100),
            (GridTrackKind::Gutter, minpx20, maxpx20),
            (GridTrackKind::Track, auto(), auto()),
            (GridTrackKind::Gutter, minpx0, maxpx0),
        ];

        assert_eq!(tracks.len(), expected.len(), "Number of tracks doesn't match");

        for (idx, (actual, (kind, min, max))) in tracks.into_iter().zip(expected).enumerate() {
            assert_eq!(actual.kind, kind, "Track {idx} (0-based index)");
            assert_eq!(actual.min_track_sizing_function, min, "Track {idx} (0-based index)");
            assert_eq!(actual.max_track_sizing_function, max, "Track {idx} (0-based index)");
        }
    }
}
