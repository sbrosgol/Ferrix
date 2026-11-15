//! Linux/BSD: the xdg-desktop-portal Print interface. The desktop shows its print dialog and
//! spools the PDF file to CUPS, which prints PDF natively.

use std::fs::File;

use ashpd::{
    Error as PortalError, WindowIdentifier,
    desktop::{
        ResponseError,
        print::{PageSetup, PreparePrintOptions, PrintOptions, PrintProxy, Settings},
    },
};

use crate::{Parent, PrintError, PrintJob, PrintOutcome};

/// Shows the desktop print dialog and submits the job. Needs a running tokio runtime.
pub async fn print_with_portal(parent: Parent, job: &PrintJob) -> Result<PrintOutcome, PrintError> {
    let proxy = PrintProxy::new()
        .await
        .map_err(|e| PrintError::Unavailable(format!("print portal: {e}")))?;
    let identifier = match parent {
        Parent::X11(xid) => Some(WindowIdentifier::from_xid(xid as std::os::raw::c_ulong)),
        Parent::None | Parent::Win32(_) => None,
    };

    let prepared = proxy
        .prepare_print(
            identifier.as_ref(),
            &job.title,
            Settings::default(),
            PageSetup::default(),
            PreparePrintOptions::default().set_modal(true),
        )
        .await
        .and_then(|request| request.response());
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(PortalError::Response(ResponseError::Cancelled)) => return Ok(PrintOutcome::Cancelled),
        Err(e) => return Err(PrintError::Platform(e.to_string())),
    };

    let file = File::open(&job.path).map_err(|e| PrintError::Platform(e.to_string()))?;
    proxy
        .print(
            identifier.as_ref(),
            &job.title,
            &file,
            PrintOptions::default()
                .set_token(prepared.token)
                .set_modal(true),
        )
        .await
        .and_then(|request| request.response())
        .map_err(|e| PrintError::Platform(e.to_string()))?;
    Ok(PrintOutcome::Submitted)
}
