//! Port of libs/librepcb/core/3d/occmodel.cpp's STEP unit handling boundary.
//! Monstertruck/step-p21 parse geometry and syntax; this adapter resolves unit
//! contexts because Monstertruck 0.4.1 does not apply them to loaded geometry.
use monstertruck_io::step::load::{
    Table,
    step_p21::{
        self,
        ast::{DataSection, EntityInstance, Name, Parameter, Record},
    },
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("STEP parse failed: {0}")]
    Parse(String),
    #[error("Expected one STEP DATA section, found {0}")]
    DataSections(usize),
    #[error("Duplicate STEP entity #{0}")]
    DuplicateEntity(u64),
    #[error("STEP entity #{id}: {detail}")]
    Entity { id: u64, detail: String },
    #[error("No length unit context for 3D geometry")]
    MissingUnits,
    #[error("Mixed length unit contexts are not supported yet")]
    MixedUnits,
}
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(id: u64, detail: impl Into<String>) -> Error {
    Error::Entity {
        id,
        detail: detail.into(),
    }
}
pub(crate) fn list(value: &Parameter, id: u64) -> Result<&[Parameter]> {
    match value {
        Parameter::List(items) => Ok(items),
        _ => Err(invalid(id, "expected parameter list")),
    }
}
pub(crate) fn reference(value: &Parameter, id: u64) -> Result<u64> {
    match value {
        Parameter::Ref(Name::Entity(target)) => Ok(*target),
        _ => Err(invalid(id, "expected entity reference")),
    }
}
pub(crate) fn number(value: &Parameter, id: u64) -> Result<f64> {
    let value = match value {
        Parameter::Real(value) => *value,
        Parameter::Integer(value) => *value as f64,
        Parameter::Typed { parameter, .. } => return number(parameter, id),
        _ => return Err(invalid(id, "expected numeric measure")),
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err(invalid(id, "non-finite number"))
    }
}

#[derive(Debug)]
pub(crate) struct Records(pub BTreeMap<u64, Vec<Record>>);
impl Records {
    pub(crate) fn new(data: &DataSection) -> Result<Self> {
        let mut records = BTreeMap::new();
        for entity in &data.entities {
            let (id, value) = match entity {
                EntityInstance::Simple { id, record } => (*id, vec![record.clone()]),
                EntityInstance::Complex { id, subsuper } => (*id, subsuper.0.clone()),
            };
            if records.insert(id, value).is_some() {
                return Err(Error::DuplicateEntity(id));
            }
        }
        Ok(Self(records))
    }
    pub(crate) fn part(&self, id: u64, name: &str) -> Option<&Record> {
        self.0.get(&id)?.iter().find(|record| record.name == name)
    }
    pub(crate) fn args(&self, id: u64, name: &str) -> Result<&[Parameter]> {
        let record = self
            .part(id, name)
            .ok_or_else(|| invalid(id, format!("missing {name}")))?;
        list(&record.parameter, id)
    }
    fn length_unit(&self, id: u64, visiting: &mut BTreeSet<u64>) -> Result<f64> {
        if !visiting.insert(id) {
            return Err(invalid(id, "cyclic unit definition"));
        }
        if visiting.len() > 64 {
            return Err(invalid(id, "unit chain exceeds 64 entities"));
        }
        if self.part(id, "LENGTH_UNIT").is_none() {
            return Err(invalid(id, "expected LENGTH_UNIT"));
        }
        let factor = if self.part(id, "SI_UNIT").is_some() {
            let [prefix, Parameter::Enumeration(unit)] = self.args(id, "SI_UNIT")? else {
                return Err(invalid(id, "invalid SI length unit"));
            };
            if unit != "METRE" {
                return Err(invalid(id, "length SI_UNIT must be METRE"));
            }
            let prefix = match prefix {
                Parameter::NotProvided => 1.,
                Parameter::Enumeration(value) => match value.as_str() {
                    "EXA" => 1e18,
                    "PETA" => 1e15,
                    "TERA" => 1e12,
                    "GIGA" => 1e9,
                    "MEGA" => 1e6,
                    "KILO" => 1e3,
                    "HECTO" => 1e2,
                    "DECA" => 1e1,
                    "DECI" => 1e-1,
                    "CENTI" => 1e-2,
                    "MILLI" => 1e-3,
                    "MICRO" => 1e-6,
                    "NANO" => 1e-9,
                    "PICO" => 1e-12,
                    "FEMTO" => 1e-15,
                    "ATTO" => 1e-18,
                    _ => return Err(invalid(id, "unsupported SI prefix")),
                },
                _ => return Err(invalid(id, "invalid SI prefix")),
            };
            prefix * 1000.
        } else {
            let [_, measure] = self.args(id, "CONVERSION_BASED_UNIT")? else {
                return Err(invalid(id, "invalid conversion-based unit"));
            };
            let measure = reference(measure, id)?;
            let name = if self.part(measure, "LENGTH_MEASURE_WITH_UNIT").is_some() {
                "LENGTH_MEASURE_WITH_UNIT"
            } else {
                "MEASURE_WITH_UNIT"
            };
            let [value, base] = self.args(measure, name)? else {
                return Err(invalid(measure, "invalid unit measure"));
            };
            number(value, measure)? * self.length_unit(reference(base, measure)?, visiting)?
        };
        visiting.remove(&id);
        if factor.is_finite() && factor > 0. {
            Ok(factor)
        } else {
            Err(invalid(id, "unit scale must be finite and positive"))
        }
    }
    fn length_to_mm(&self) -> Result<f64> {
        let mut scale: Option<f64> = None;
        for &id in self.0.keys() {
            let Some(geometric) = self.part(id, "GEOMETRIC_REPRESENTATION_CONTEXT") else {
                continue;
            };
            if list(&geometric.parameter, id)? != [Parameter::Integer(3)] {
                continue;
            }
            let [units] = self.args(id, "GLOBAL_UNIT_ASSIGNED_CONTEXT")? else {
                return Err(invalid(id, "invalid unit context"));
            };
            let mut length = None;
            for unit in list(units, id)? {
                let unit = reference(unit, id)?;
                if self.part(unit, "LENGTH_UNIT").is_some() {
                    if length.is_some() {
                        return Err(invalid(id, "multiple length units in one context"));
                    }
                    length = Some(self.length_unit(unit, &mut BTreeSet::new())?);
                }
                if self.part(unit, "PLANE_ANGLE_UNIT").is_some() {
                    let args = self.args(unit, "SI_UNIT")?;
                    if args
                        != [
                            Parameter::NotProvided,
                            Parameter::Enumeration("RADIAN".into()),
                        ]
                    {
                        return Err(invalid(unit, "only radian angle contexts are supported"));
                    }
                }
            }
            let length = length.ok_or_else(|| invalid(id, "missing context length unit"))?;
            if let Some(previous) = scale
                && (previous - length).abs() > previous.abs() * 1e-12
            {
                return Err(Error::MixedUnits);
            }
            scale = Some(length);
        }
        scale.ok_or(Error::MissingUnits)
    }
}

pub(crate) fn text(value: &Parameter, id: u64) -> Result<&str> {
    match value {
        Parameter::String(value) => Ok(value),
        _ => Err(invalid(id, "expected string")),
    }
}

pub struct StepDocument {
    pub table: Table,
    pub length_to_mm: f64,
    pub(crate) records: Records,
}
static_assertions::assert_impl_all!(StepDocument: Send, Sync);
impl StepDocument {
    pub fn entity_count(&self) -> usize {
        self.records.0.len()
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let decoded;
        let source = match std::str::from_utf8(bytes) {
            Ok(source) => source,
            Err(_) => {
                decoded = bytes
                    .iter()
                    .map(|&byte| char::from(byte))
                    .collect::<String>();
                &decoded
            }
        };
        let exchange =
            step_p21::parser::parse(source).map_err(|error| Error::Parse(error.to_string()))?;
        if exchange.data.len() != 1 {
            return Err(Error::DataSections(exchange.data.len()));
        }
        let data = &exchange.data[0];
        let records = Records::new(data)?;
        let length_to_mm = records.length_to_mm()?;
        Ok(Self {
            table: Table::from_data_section(data),
            length_to_mm,
            records,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn records(body: &str) -> Records {
        let data: DataSection = format!("DATA;{body}ENDSEC;").parse().unwrap();
        Records::new(&data).unwrap()
    }
    #[test]
    fn si_and_recursive_conversion_units() {
        for (prefix, scale) in [
            ("$", 1000.),
            (".MILLI.", 1.),
            (".CENTI.", 10.),
            (".MICRO.", 0.001),
        ] {
            let data = records(&format!(
                "#1=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT({prefix},.METRE.));"
            ));
            assert!((data.length_unit(1, &mut BTreeSet::new()).unwrap() - scale).abs() < 1e-12);
        }
        let data = records(
            "#1=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));#2=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#1);#3=(CONVERSION_BASED_UNIT('inch',#2) LENGTH_UNIT() NAMED_UNIT(*));#4=MEASURE_WITH_UNIT(LENGTH_MEASURE(12.),#3);#5=(CONVERSION_BASED_UNIT('foot',#4) LENGTH_UNIT() NAMED_UNIT(*));",
        );
        assert_eq!(data.length_unit(3, &mut BTreeSet::new()).unwrap(), 25.4);
        assert!((data.length_unit(5, &mut BTreeSet::new()).unwrap() - 304.8).abs() < 1e-10);
    }
    #[test]
    fn cycles_missing_units_and_mixed_contexts_fail() {
        let cyclic = records(
            "#1=(CONVERSION_BASED_UNIT('bad',#2) LENGTH_UNIT() NAMED_UNIT(*));#2=MEASURE_WITH_UNIT(LENGTH_MEASURE(1.),#1);",
        );
        assert!(cyclic.length_unit(1, &mut BTreeSet::new()).is_err());
        assert!(matches!(
            records("").length_to_mm(),
            Err(Error::MissingUnits)
        ));
        let mixed = records(
            "#1=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));#2=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.));#3=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#1)) REPRESENTATION_CONTEXT('',''));#4=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#2)) REPRESENTATION_CONTEXT('',''));",
        );
        assert!(matches!(mixed.length_to_mm(), Err(Error::MixedUnits)));
    }
}
