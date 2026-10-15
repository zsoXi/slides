//! Dense labels never silently promise readable placement.
use serde_json::json;
use slides::{Input, Options, run};

type Result = std::result::Result<(), Box<dyn std::error::Error>>;
#[test]
fn dense_and_long_numeric_labels_name_the_slide_and_element() -> Result {
    let dir = tempfile::tempdir()?;
    for (labels, values) in [
        (
            (0..21).map(|i| format!("Category {i}")).collect::<Vec<_>>(),
            vec![1.; 21],
        ),
        (vec!["A".into()], vec![1.2345678901234568e30]),
    ] {
        let request = serde_json::from_value::<Input>(
            json!({"op":"create","output":"dense.pptx","slides":[{"title":"Control"},{"title":"Dense chart","chart":{"type":"line","labels":labels,"series":[{"name":"Measure","values":values}]}}]}),
        )?;
        let output = run(&request, &Options::new(dir.path()))?;
        let warning = output
            .warnings
            .iter()
            .find(|w| w.code == "chart.labels_review")
            .ok_or("no density warning")?;
        assert_eq!(warning.params["slide"], "2");
        assert_eq!(warning.params["element"], "blocks/0/chart");
    }
    Ok(())
}
