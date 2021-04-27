// Copyright (C) 2019-2021 Aleo Systems Inc.
// This file is part of the Leo library.

// The Leo library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The Leo library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the Leo library. If not, see <https://www.gnu.org/licenses/>.

use anyhow::{anyhow, Error, Result};
use reqwest::{
    blocking::{multipart::Form, Client, Response},
    Method,
    StatusCode,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

/// Format to use.
/// Default is JSON, but publish route uses FormData
#[derive(Clone, Debug)]
pub enum ContentType {
    JSON,
    FormData,
}

/// API Routes and Request bodies.
/// Structs that implement Route MUST also support Serialize to be usable in Api::run_route(r: Route)
pub trait Route {
    /// [`true`] if a route supports bearer authentication.
    /// For example, the login route.
    const AUTH: bool;

    /// The HTTP method to use when requesting.
    const METHOD: Method;

    /// The URL path without the first forward slash (e.g. v1/package/fetch)
    const PATH: &'static str;

    /// Content type: JSON or Multipart/FormData. Only usable in POST/PUT queries.
    const CONTENT_TYPE: ContentType;

    /// The output type for this route. For example, the login route output is [`String`].
    /// But for other routes may be more complex.
    type Output;

    /// Process the reqwest Response and turn it into an Output.
    fn process(&self, res: Response) -> Result<Self::Output>;

    /// Represent self as a form data for multipart (ContentType::FormData) requests.
    fn to_form(&self) -> Option<Form> {
        None
    }

    /// Transform specific status codes into correct errors for this route.
    /// For example 404 on package fetch should mean that 'Package is not found'
    fn status_to_err(&self, _status: StatusCode) -> Error {
        anyhow!("Unidentified API error")
    }
}

/// REST API handler with reqwest::blocking inside.
#[derive(Clone, Debug)]
pub struct Api {
    host: String,
    client: Client,
    /// Authorization token for API requests.
    auth_token: Option<String>,
}

impl Api {
    /// Returns a new instance of API.
    /// The set host and Client are created automatically.
    pub fn new(host: String, auth_token: Option<String>) -> Api {
        Api {
            client: Client::new(),
            auth_token,
            host,
        }
    }

    pub fn host(&self) -> &str {
        &*self.host
    }

    /// Returns the token for bearer auth, otherwise None.
    /// The [`auth_token`] should be passed into the Api through Context.
    pub fn auth_token(&self) -> Option<String> {
        self.auth_token.clone()
    }

    /// Set the authorization token for future requests.
    pub fn set_auth_token(&mut self, token: String) {
        self.auth_token = Some(token);
    }

    /// Run specific route struct. Turn struct into request body
    /// and use type constants and Route implementation to get request params.
    pub fn run_route<T>(&self, route: T) -> Result<T::Output>
    where
        T: Route,
        T: Serialize,
    {
        let mut res = self.client.request(T::METHOD, &format!("{}{}", self.host, T::PATH));

        // add body for POST and PUT requests
        if T::METHOD == Method::POST || T::METHOD == Method::PUT {
            res = match T::CONTENT_TYPE {
                ContentType::JSON => res.json(&route),
                ContentType::FormData => {
                    let form = route
                        .to_form()
                        .unwrap_or_else(|| unimplemented!("to_form is not implemented for this route"));

                    res.multipart(form)
                }
            }
        };

        // if Route::Auth is true and token is present - pass it
        if T::AUTH && self.auth_token().is_some() {
            res = res.bearer_auth(&self.auth_token().unwrap());
        };

        // only one error is possible here
        let res = res.send().map_err(|_| {
            anyhow!("Unable to connect to Aleo PM. If you specified custom API endpoint, then check the URL for errors")
        })?;

        // where magic begins
        route.process(res)
    }
}

// --------------------------------------------------
// |                Defining routes                 |
// --------------------------------------------------

/// Handler for 'fetch' route - fetch packages from Aleo PM
/// Route: POST /v1/package/fetch
#[derive(Serialize, Debug)]
pub struct Fetch {
    pub author: String,
    pub package_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Route for Fetch {
    type Output = Response;

    const AUTH: bool = true;
    const CONTENT_TYPE: ContentType = ContentType::JSON;
    const METHOD: Method = Method::POST;
    const PATH: &'static str = "api/package/fetch";

    fn process(&self, res: Response) -> Result<Self::Output> {
        // check status code first
        if res.status() != 200 {
            return Err(self.status_to_err(res.status()));
        };

        Ok(res)
    }

    fn status_to_err(&self, status: StatusCode) -> Error {
        match status {
            StatusCode::BAD_REQUEST => anyhow!("Package is not found - check author and/or package name"),
            // TODO: we should return 404 on not found author/package
            // and return BAD_REQUEST if data format is incorrect or some of the arguments
            // were not passed
            StatusCode::NOT_FOUND => anyhow!("Package not found"),
            _ => anyhow!("Unknown API error: {}", status),
        }
    }
}

/// Handler for 'login' route - send username and password and receive JWT
/// Route: POST /v1/account/authenticate
#[derive(Serialize)]
pub struct Login {
    pub email_username: String,
    pub password: String,
}

impl Route for Login {
    type Output = Response;

    const AUTH: bool = false;
    const CONTENT_TYPE: ContentType = ContentType::JSON;
    const METHOD: Method = Method::POST;
    const PATH: &'static str = "api/account/authenticate";

    fn process(&self, res: Response) -> Result<Self::Output> {
        if res.status() != 200 {
            return Err(self.status_to_err(res.status()));
        }

        Ok(res)
    }

    fn status_to_err(&self, status: StatusCode) -> Error {
        match status {
            StatusCode::BAD_REQUEST => anyhow!("This username is not yet registered or the password is incorrect"),
            // TODO: NOT_FOUND here should be replaced, this error code has no relation to what this route is doing
            StatusCode::NOT_FOUND => anyhow!("Incorrect password"),
            _ => anyhow!("Unknown API error: {}", status),
        }
    }
}

#[derive(Serialize)]
pub struct Publish {
    pub name: String,
    pub remote: String,
    pub version: String,
    pub file: PathBuf,
}

#[derive(Deserialize)]
pub struct PublishResponse {
    package_id: String,
}

impl Route for Publish {
    type Output = String;

    const AUTH: bool = true;
    const CONTENT_TYPE: ContentType = ContentType::FormData;
    const METHOD: Method = Method::POST;
    const PATH: &'static str = "api/package/publish";

    fn to_form(&self) -> Option<Form> {
        Form::new()
            .text("name", self.name.clone())
            .text("remote", self.remote.clone())
            .text("version", self.version.clone())
            .file("file", self.file.clone())
            .ok()
    }

    fn process(&self, res: Response) -> Result<Self::Output> {
        let status = res.status();

        if status == StatusCode::OK {
            let body: PublishResponse = res.json()?;
            Ok(body.package_id)
        } else {
            let res: HashMap<String, String> = res.json()?;
            Err(match status {
                StatusCode::BAD_REQUEST => anyhow!("{}", res.get("message").unwrap()),
                StatusCode::UNAUTHORIZED => anyhow!("You are not logged in. Please use `leo login` to login"),
                StatusCode::FAILED_DEPENDENCY => anyhow!("This package version is already published"),
                StatusCode::INTERNAL_SERVER_ERROR => {
                    anyhow!("Server error, please contact us at https://github.com/AleoHQ/leo/issues")
                }
                _ => anyhow!("Unknown status code"),
            })
        }
    }
}

/// Handler for 'my_profile' route. Meant to be used to get profile details but
/// in the current application it is used to check if the user is logged in. Any non-200 response
/// is treated as Unauthorized.
#[derive(Serialize)]
pub struct Profile {}

#[derive(Deserialize)]
pub struct ProfileResponse {
    username: String,
}

impl Route for Profile {
    // Some with Username is success, None is failure.
    type Output = Option<String>;

    const AUTH: bool = true;
    const CONTENT_TYPE: ContentType = ContentType::JSON;
    const METHOD: Method = Method::GET;
    const PATH: &'static str = "api/account/my_profile";

    fn process(&self, res: Response) -> Result<Self::Output> {
        // this may be extended for more precise error handling
        let status = res.status();
        if status == StatusCode::OK {
            let body: ProfileResponse = res.json()?;
            return Ok(Some(body.username));
        }

        Ok(None)
    }
}
