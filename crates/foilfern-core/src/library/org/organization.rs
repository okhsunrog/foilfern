//! Port of libs/librepcb/core/library/org/organization.{h,cpp}.
//!
//! Differences to upstream:
//! - The URL is stored verbatim as string instead of a `QUrl` (see
//!   COMPAT.md).
//! - `getLogoPixmap()` is not ported (UI); [`Organization::logo_png()`]
//!   returns the PNG file content.
//! - `duplicate_from()` copies the output jobs (with new UUIDs); upstream
//!   clears the job lists before iterating them and thus drops all jobs.

use std::collections::BTreeMap;

use super::board_design_rule_check_settings::load_options;
use super::organization_check::run_organization_checks;
use super::organization_pcb_design_rules::OrganizationPcbDesignRules;
use crate::fileio::{FileSystem, TransactionalDirectory};
use crate::geometry::property;
use crate::job::OutputJob;
use crate::library::{
    BaseMetadata, LibraryBaseElement, LibraryCheckMessage, Result, save_element_files,
};
use crate::serialization::{DeserializeObject, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// File name of the logo.
const LOGO_FILE_NAME: &str = "logo.png";

/// An organization, e.g. a PCB manufacturer or an assembly house.
#[derive(Debug)]
pub struct Organization {
    directory: TransactionalDirectory,
    metadata: BaseMetadata,
    logo_png: Vec<u8>,
    url: String,
    country: String,
    fabs: Vec<String>,
    shipping: Vec<String>,
    is_sponsor: bool,
    priority: i32,
    pcb_design_rules: Vec<OrganizationPcbDesignRules>,
    pcb_output_jobs: Vec<OutputJob>,
    assembly_output_jobs: Vec<OutputJob>,
    user_output_jobs: Vec<OutputJob>,
    // Arbitrary options for forward compatibility in case we really need to
    // add new settings in a minor release.
    options: BTreeMap<String, Vec<SExpression>>,
}

static_assertions::assert_impl_all!(Organization: Send, Sync);

impl Organization {
    /// Creates a new organization in a temporary directory.
    pub fn new(metadata: BaseMetadata) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            metadata,
            logo_png: Vec::new(),
            url: String::new(),
            country: String::new(),
            fabs: vec![String::new()],
            shipping: vec![String::new()],
            is_sponsor: false,
            priority: 0,
            pcb_design_rules: Vec::new(),
            pcb_output_jobs: Vec::new(),
            assembly_output_jobs: Vec::new(),
            user_output_jobs: Vec::new(),
            options: BTreeMap::new(),
        })
    }

    property!(
        /// Returns the logo (PNG file content, empty if there is no logo).
        ref logo_png: Vec<u8>, set_logo_png
    );
    property!(
        /// Returns the URL of the organization's website (may be empty).
        ref url: String, set_url
    );
    property!(
        /// Returns the country code (e.g. `"CH"`, may be empty).
        ref country: String, set_country
    );
    property!(
        /// Returns the countries of the fabs (e.g. `["CN"]`; stored as
        /// comma separated list, an empty list is `[""]`).
        ref fabs: Vec<String>, set_fabs
    );
    property!(
        /// Returns the shipping destinations (e.g. `["worldwide"]`; stored as
        /// comma separated list, an empty list is `[""]`).
        ref shipping: Vec<String>, set_shipping
    );
    property!(
        /// Returns whether the organization sponsors LibrePCB.
        copy is_sponsor: bool, set_sponsor
    );
    property!(
        /// Returns the priority to influence the sort order of organizations
        /// (100: LibrePCB Fab, 50..99: user-created organizations, 1..49:
        /// important organizations like sponsors, 0: any other).
        copy priority: i32, set_priority
    );
    property!(
        /// Returns the PCB design rules.
        ref pcb_design_rules: Vec<OrganizationPcbDesignRules>, set_pcb_design_rules
    );
    property!(
        /// Returns the PCB output jobs (`pcb_job` nodes).
        ref pcb_output_jobs: Vec<OutputJob>, set_pcb_output_jobs
    );
    property!(
        /// Returns the assembly output jobs (`assembly_job` nodes).
        ref assembly_output_jobs: Vec<OutputJob>, set_assembly_output_jobs
    );
    property!(
        /// Returns the user output jobs (`user_job` nodes).
        ref user_output_jobs: Vec<OutputJob>, set_user_output_jobs
    );

    /// Returns the PCB design rules with the given UUID.
    pub fn pcb_design_rules_by_uuid(&self, uuid: Uuid) -> Option<&OrganizationPcbDesignRules> {
        self.pcb_design_rules.iter().find(|r| r.uuid() == uuid)
    }

    /// Returns the first PCB output job of the given type (e.g.
    /// `"gerber_excellon"`, upstream `findPcbOutputJob()`).
    pub fn pcb_output_job_by_type(&self, job_type: &str) -> Option<&OutputJob> {
        self.pcb_output_jobs
            .iter()
            .find(|job| job.type_name() == job_type)
    }

    /// Makes this organization a copy of `other` with new UUIDs of the PCB
    /// design rules and output jobs (but keeps the organization UUID),
    /// removing all files of this organization (upstream `duplicateFrom()`).
    pub fn duplicate_from(&mut self, other: &Organization) -> Result<()> {
        self.directory.remove_dir_recursively("")?;
        self.metadata.duplicate_from(&other.metadata);
        self.logo_png = other.logo_png.clone();
        self.url = other.url.clone();
        self.country = other.country.clone();
        self.fabs = other.fabs.clone();
        self.shipping = other.shipping.clone();
        self.is_sponsor = other.is_sponsor;
        self.priority = other.priority;
        self.options = other.options.clone();
        self.pcb_design_rules = other
            .pcb_design_rules
            .iter()
            .map(|rules| {
                let mut copy = rules.clone();
                copy.set_uuid(Uuid::new_random());
                copy
            })
            .collect();
        let copy_jobs = |jobs: &[OutputJob]| -> Vec<OutputJob> {
            jobs.iter()
                .map(|job| job.with_uuid(Uuid::new_random()))
                .collect()
        };
        self.pcb_output_jobs = copy_jobs(&other.pcb_output_jobs);
        self.assembly_output_jobs = copy_jobs(&other.assembly_output_jobs);
        self.user_output_jobs = copy_jobs(&other.user_output_jobs);
        Ok(())
    }
}

impl LibraryBaseElement for Organization {
    const SHORT_ELEMENT_NAME: &'static str = "org";
    const LONG_ELEMENT_NAME: &'static str = "organization";

    fn metadata(&self) -> &BaseMetadata {
        &self.metadata
    }
    fn metadata_mut(&mut self) -> &mut BaseMetadata {
        &mut self.metadata
    }
    fn directory(&self) -> &TransactionalDirectory {
        &self.directory
    }
    fn directory_mut(&mut self) -> &mut TransactionalDirectory {
        &mut self.directory
    }

    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self> {
        let split = |s: String| -> Vec<String> { s.split(',').map(str::to_owned).collect() };
        Ok(Self {
            metadata: BaseMetadata::deserialize(root)?,
            logo_png: directory
                .read_if_exists(LOGO_FILE_NAME)?
                .unwrap_or_default(),
            url: root.child_value("url/@0")?,
            country: root.child_value("country/@0")?,
            fabs: split(root.child_value("fabs/@0")?),
            shipping: split(root.child_value("shipping/@0")?),
            is_sponsor: root.child_value("sponsor/@0")?,
            priority: root.child_value("priority/@0")?,
            pcb_design_rules: root
                .children_named("pcb_design_rules")
                .map(OrganizationPcbDesignRules::deserialize)
                .collect::<crate::serialization::Result<_>>()?,
            pcb_output_jobs: load_jobs(root, "pcb_job")?,
            assembly_output_jobs: load_jobs(root, "assembly_job")?,
            user_output_jobs: load_jobs(root, "user_job")?,
            options: load_options(root)?,
            directory,
        })
    }

    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>> {
        let mut msgs = Vec::new();
        run_organization_checks(self, &mut msgs);
        Ok(msgs)
    }

    fn save(&mut self) -> Result<()> {
        save_element_files(self)?;
        if self.logo_png.is_empty() {
            self.directory.remove_file(LOGO_FILE_NAME)?;
        } else {
            self.directory.write(LOGO_FILE_NAME, &self.logo_png)?;
        }
        Ok(())
    }
}

impl SerializeObject for Organization {
    fn serialize(&self, root: &mut List) {
        self.metadata.serialize(root);
        root.ensure_line_break();
        root.append_child("url", &self.url);
        root.ensure_line_break();
        root.append_child("country", &self.country);
        root.ensure_line_break();
        root.append_child("fabs", &self.fabs.join(","));
        root.ensure_line_break();
        root.append_child("shipping", &self.shipping.join(","));
        root.ensure_line_break();
        root.append_child("sponsor", &self.is_sponsor);
        root.ensure_line_break();
        root.append_child("priority", &self.priority);
        root.ensure_line_break();
        for rules in &self.pcb_design_rules {
            rules.serialize(root.append_list("pcb_design_rules"));
            root.ensure_line_break();
        }
        let jobs = [
            ("pcb_job", &self.pcb_output_jobs),
            ("assembly_job", &self.assembly_output_jobs),
            ("user_job", &self.user_output_jobs),
        ];
        for (name, jobs) in jobs {
            for job in jobs {
                job.serialize(root.append_list(name));
                root.ensure_line_break();
            }
        }
        for node in self.options.values().flatten() {
            root.push(node.clone());
            root.ensure_line_break();
        }
        self.metadata.serialize_message_approvals(root);
        root.ensure_line_break();
    }
}

/// Loads the output jobs stored as `name` children of `root`.
fn load_jobs(root: &SExpression, name: &str) -> crate::serialization::Result<Vec<OutputJob>> {
    root.children_named(name)
        .map(OutputJob::deserialize)
        .collect()
}
