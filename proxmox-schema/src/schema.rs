//! Data types to decscribe data types.
//!
//! This is loosly based on JSON Schema, but uses static RUST data
//! types. This way we can build completely static API
//! definitions included with the programs read-only text segment.

use std::fmt;

use anyhow::{bail, format_err, Error};
use serde_json::{json, Value};

use crate::ConstRegexPattern;

/// Error type for schema validation
///
/// The validation functions may produce several error message,
/// i.e. when validation objects, it can produce one message for each
/// erroneous object property.
#[derive(Default, Debug)]
pub struct ParameterError {
    error_list: Vec<(String, Error)>,
}

impl std::error::Error for ParameterError {}

impl ParameterError {
    pub fn new() -> Self {
        Self {
            error_list: Vec::new(),
        }
    }

    pub fn push(&mut self, name: String, value: Error) {
        self.error_list.push((name, value));
    }

    pub fn len(&self) -> usize {
        self.error_list.len()
    }

    pub fn errors(&self) -> &[(String, Error)] {
        &self.error_list
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn add_errors(&mut self, prefix: &str, err: Error) {
        if let Some(param_err) = err.downcast_ref::<ParameterError>() {
            for (sub_key, sub_err) in param_err.errors().iter() {
                self.push(
                    format!("{}/{}", prefix, sub_key),
                    format_err!("{}", sub_err),
                );
            }
        } else {
            self.push(prefix.to_string(), err);
        }
    }
}

impl fmt::Display for ParameterError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut msg = String::new();

        if !self.is_empty() {
            msg.push_str("parameter verification errors\n\n");
        }

        for (name, err) in self.error_list.iter() {
            msg.push_str(&format!("parameter '{}': {}\n", name, err));
        }

        write!(f, "{}", msg)
    }
}

/// Data type to describe boolean values
#[derive(Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct BooleanSchema {
    pub description: &'static str,
    /// Optional default value.
    pub default: Option<bool>,
}

impl BooleanSchema {
    pub const fn new(description: &'static str) -> Self {
        BooleanSchema {
            description,
            default: None,
        }
    }

    pub const fn default(mut self, default: bool) -> Self {
        self.default = Some(default);
        self
    }

    pub const fn schema(self) -> Schema {
        Schema::Boolean(self)
    }

    /// Verify JSON value using a `BooleanSchema`.
    pub fn verify_json(&self, data: &Value) -> Result<(), Error> {
        if !data.is_boolean() {
            bail!("Expected boolean value.");
        }
        Ok(())
    }
}

/// Data type to describe integer values.
#[derive(Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct IntegerSchema {
    pub description: &'static str,
    /// Optional minimum.
    pub minimum: Option<isize>,
    /// Optional maximum.
    pub maximum: Option<isize>,
    /// Optional default.
    pub default: Option<isize>,
}

impl IntegerSchema {
    pub const fn new(description: &'static str) -> Self {
        IntegerSchema {
            description,
            default: None,
            minimum: None,
            maximum: None,
        }
    }

    pub const fn default(mut self, default: isize) -> Self {
        self.default = Some(default);
        self
    }

    pub const fn minimum(mut self, minimum: isize) -> Self {
        self.minimum = Some(minimum);
        self
    }

    pub const fn maximum(mut self, maximium: isize) -> Self {
        self.maximum = Some(maximium);
        self
    }

    pub const fn schema(self) -> Schema {
        Schema::Integer(self)
    }

    fn check_constraints(&self, value: isize) -> Result<(), Error> {
        if let Some(minimum) = self.minimum {
            if value < minimum {
                bail!(
                    "value must have a minimum value of {} (got {})",
                    minimum,
                    value
                );
            }
        }

        if let Some(maximum) = self.maximum {
            if value > maximum {
                bail!(
                    "value must have a maximum value of {} (got {})",
                    maximum,
                    value
                );
            }
        }

        Ok(())
    }

    /// Verify JSON value using an `IntegerSchema`.
    pub fn verify_json(&self, data: &Value) -> Result<(), Error> {
        if let Some(value) = data.as_i64() {
            self.check_constraints(value as isize)
        } else {
            bail!("Expected integer value.");
        }
    }
}

/// Data type to describe (JSON like) number value
#[derive(Debug)]
pub struct NumberSchema {
    pub description: &'static str,
    /// Optional minimum.
    pub minimum: Option<f64>,
    /// Optional maximum.
    pub maximum: Option<f64>,
    /// Optional default.
    pub default: Option<f64>,
}

impl NumberSchema {
    pub const fn new(description: &'static str) -> Self {
        NumberSchema {
            description,
            default: None,
            minimum: None,
            maximum: None,
        }
    }

    pub const fn default(mut self, default: f64) -> Self {
        self.default = Some(default);
        self
    }

    pub const fn minimum(mut self, minimum: f64) -> Self {
        self.minimum = Some(minimum);
        self
    }

    pub const fn maximum(mut self, maximium: f64) -> Self {
        self.maximum = Some(maximium);
        self
    }

    pub const fn schema(self) -> Schema {
        Schema::Number(self)
    }

    fn check_constraints(&self, value: f64) -> Result<(), Error> {
        if let Some(minimum) = self.minimum {
            if value < minimum {
                bail!(
                    "value must have a minimum value of {} (got {})",
                    minimum,
                    value
                );
            }
        }

        if let Some(maximum) = self.maximum {
            if value > maximum {
                bail!(
                    "value must have a maximum value of {} (got {})",
                    maximum,
                    value
                );
            }
        }

        Ok(())
    }

    /// Verify JSON value using an `NumberSchema`.
    pub fn verify_json(&self, data: &Value) -> Result<(), Error> {
        if let Some(value) = data.as_f64() {
            self.check_constraints(value)
        } else {
            bail!("Expected number value.");
        }
    }
}

#[cfg(feature = "test-harness")]
impl Eq for NumberSchema {}

#[cfg(feature = "test-harness")]
impl PartialEq for NumberSchema {
    fn eq(&self, rhs: &Self) -> bool {
        fn f64_eq(l: Option<f64>, r: Option<f64>) -> bool {
            match (l, r) {
                (None, None) => true,
                (Some(l), Some(r)) => (l - r).abs() < 0.0001,
                _ => false,
            }
        }

        self.description == rhs.description
            && f64_eq(self.minimum, rhs.minimum)
            && f64_eq(self.maximum, rhs.maximum)
            && f64_eq(self.default, rhs.default)
    }
}

/// Data type to describe string values.
#[derive(Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct StringSchema {
    pub description: &'static str,
    /// Optional default value.
    pub default: Option<&'static str>,
    /// Optional minimal length.
    pub min_length: Option<usize>,
    /// Optional maximal length.
    pub max_length: Option<usize>,
    /// Optional microformat.
    pub format: Option<&'static ApiStringFormat>,
    /// A text representation of the format/type (used to generate documentation).
    pub type_text: Option<&'static str>,
}

impl StringSchema {
    pub const fn new(description: &'static str) -> Self {
        StringSchema {
            description,
            default: None,
            min_length: None,
            max_length: None,
            format: None,
            type_text: None,
        }
    }

    pub const fn default(mut self, text: &'static str) -> Self {
        self.default = Some(text);
        self
    }

    pub const fn format(mut self, format: &'static ApiStringFormat) -> Self {
        self.format = Some(format);
        self
    }

    pub const fn type_text(mut self, type_text: &'static str) -> Self {
        self.type_text = Some(type_text);
        self
    }

    pub const fn min_length(mut self, min_length: usize) -> Self {
        self.min_length = Some(min_length);
        self
    }

    pub const fn max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    pub const fn schema(self) -> Schema {
        Schema::String(self)
    }

    fn check_length(&self, length: usize) -> Result<(), Error> {
        if let Some(min_length) = self.min_length {
            if length < min_length {
                bail!("value must be at least {} characters long", min_length);
            }
        }

        if let Some(max_length) = self.max_length {
            if length > max_length {
                bail!("value may only be {} characters long", max_length);
            }
        }

        Ok(())
    }

    pub fn check_constraints(&self, value: &str) -> Result<(), Error> {
        self.check_length(value.chars().count())?;

        if let Some(ref format) = self.format {
            match format {
                ApiStringFormat::Pattern(regex) => {
                    if !(regex.regex_obj)().is_match(value) {
                        bail!("value does not match the regex pattern");
                    }
                }
                ApiStringFormat::Enum(variants) => {
                    if !variants.iter().any(|e| e.value == value) {
                        bail!("value '{}' is not defined in the enumeration.", value);
                    }
                }
                ApiStringFormat::PropertyString(subschema) => {
                    subschema.parse_property_string(value)?;
                }
                ApiStringFormat::VerifyFn(verify_fn) => {
                    verify_fn(value)?;
                }
            }
        }

        Ok(())
    }

    /// Verify JSON value using this `StringSchema`.
    pub fn verify_json(&self, data: &Value) -> Result<(), Error> {
        if let Some(value) = data.as_str() {
            self.check_constraints(value)
        } else {
            bail!("Expected string value.");
        }
    }
}

/// Data type to describe array of values.
///
/// All array elements are of the same type, as defined in the `items`
/// schema.
#[derive(Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct ArraySchema {
    pub description: &'static str,
    /// Element type schema.
    pub items: &'static Schema,
    /// Optional minimal length.
    pub min_length: Option<usize>,
    /// Optional maximal length.
    pub max_length: Option<usize>,
}

impl ArraySchema {
    pub const fn new(description: &'static str, item_schema: &'static Schema) -> Self {
        ArraySchema {
            description,
            items: item_schema,
            min_length: None,
            max_length: None,
        }
    }

    pub const fn min_length(mut self, min_length: usize) -> Self {
        self.min_length = Some(min_length);
        self
    }

    pub const fn max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    pub const fn schema(self) -> Schema {
        Schema::Array(self)
    }

    fn check_length(&self, length: usize) -> Result<(), Error> {
        if let Some(min_length) = self.min_length {
            if length < min_length {
                bail!("array must contain at least {} elements", min_length);
            }
        }

        if let Some(max_length) = self.max_length {
            if length > max_length {
                bail!("array may only contain {} elements", max_length);
            }
        }

        Ok(())
    }

    /// Verify JSON value using an `ArraySchema`.
    pub fn verify_json(&self, data: &Value) -> Result<(), Error> {
        let list = match data {
            Value::Array(ref list) => list,
            Value::Object(_) => bail!("Expected array - got object."),
            _ => bail!("Expected array - got scalar value."),
        };

        self.check_length(list.len())?;

        for (i, item) in list.iter().enumerate() {
            let result = self.items.verify_json(item);
            if let Err(err) = result {
                let mut errors = ParameterError::new();
                errors.add_errors(&format!("[{}]", i), err);
                return Err(errors.into());
            }
        }

        Ok(())
    }
}

/// Property entry in an object schema:
///
/// - `name`: The name of the property
/// - `optional`: Set when the property is optional
/// - `schema`: Property type schema
pub type SchemaPropertyEntry = (&'static str, bool, &'static Schema);

/// Lookup table to Schema properties
///
/// Stores a sorted list of `(name, optional, schema)` tuples:
///
/// - `name`: The name of the property
/// - `optional`: Set when the property is optional
/// - `schema`: Property type schema
///
/// **Note:** The list has to be storted by name, because we use
/// a binary search to find items.
///
/// This is a workaround unless RUST can const_fn `Hash::new()`
pub type SchemaPropertyMap = &'static [SchemaPropertyEntry];

/// Data type to describe objects (maps).
#[derive(Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct ObjectSchema {
    pub description: &'static str,
    /// If set, allow additional properties which are not defined in
    /// the schema.
    pub additional_properties: bool,
    /// Property schema definitions.
    pub properties: SchemaPropertyMap,
    /// Default key name - used by `parse_parameter_string()`
    pub default_key: Option<&'static str>,
}

impl ObjectSchema {
    pub const fn new(description: &'static str, properties: SchemaPropertyMap) -> Self {
        ObjectSchema {
            description,
            properties,
            additional_properties: false,
            default_key: None,
        }
    }

    pub const fn additional_properties(mut self, additional_properties: bool) -> Self {
        self.additional_properties = additional_properties;
        self
    }

    pub const fn default_key(mut self, key: &'static str) -> Self {
        self.default_key = Some(key);
        self
    }

    pub const fn schema(self) -> Schema {
        Schema::Object(self)
    }

    pub fn lookup(&self, key: &str) -> Option<(bool, &Schema)> {
        if let Ok(ind) = self
            .properties
            .binary_search_by_key(&key, |(name, _, _)| name)
        {
            let (_name, optional, prop_schema) = self.properties[ind];
            Some((optional, prop_schema))
        } else {
            None
        }
    }

    /// Parse key/value pairs and verify with object schema
    ///
    /// - `test_required`: is set, checks if all required properties are
    ///   present.
    pub fn parse_parameter_strings(
        &'static self,
        data: &[(String, String)],
        test_required: bool,
    ) -> Result<Value, ParameterError> {
        ParameterSchema::from(self).parse_parameter_strings(data, test_required)
    }
}

/// Combines multiple *object* schemas into one.
///
/// Note that these are limited to object schemas. Other schemas will produce errors.
///
/// Technically this could also contain an `additional_properties` flag, however, in the JSON
/// Schema, this is not supported, so here we simply assume additional properties to be allowed.
#[derive(Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct AllOfSchema {
    pub description: &'static str,

    /// The parameter is checked against all of the schemas in the list. Note that all schemas must
    /// be object schemas.
    pub list: &'static [&'static Schema],
}

impl AllOfSchema {
    pub const fn new(description: &'static str, list: &'static [&'static Schema]) -> Self {
        Self { description, list }
    }

    pub const fn schema(self) -> Schema {
        Schema::AllOf(self)
    }

    pub fn lookup(&self, key: &str) -> Option<(bool, &Schema)> {
        for entry in self.list {
            match entry {
                Schema::AllOf(s) => {
                    if let Some(v) = s.lookup(key) {
                        return Some(v);
                    }
                }
                Schema::Object(s) => {
                    if let Some(v) = s.lookup(key) {
                        return Some(v);
                    }
                }
                _ => panic!("non-object-schema in `AllOfSchema`"),
            }
        }

        None
    }

    /// Parse key/value pairs and verify with object schema
    ///
    /// - `test_required`: is set, checks if all required properties are
    ///   present.
    pub fn parse_parameter_strings(
        &'static self,
        data: &[(String, String)],
        test_required: bool,
    ) -> Result<Value, ParameterError> {
        ParameterSchema::from(self).parse_parameter_strings(data, test_required)
    }
}

/// Beside [`ObjectSchema`] we also have an [`AllOfSchema`] which also represents objects.
pub trait ObjectSchemaType {
    fn description(&self) -> &'static str;
    fn lookup(&self, key: &str) -> Option<(bool, &Schema)>;
    fn properties(&self) -> ObjectPropertyIterator;
    fn additional_properties(&self) -> bool;

    /// Verify JSON value using an object schema.
    fn verify_json(&self, data: &Value) -> Result<(), Error> {
        let map = match data {
            Value::Object(ref map) => map,
            Value::Array(_) => bail!("Expected object - got array."),
            _ => bail!("Expected object - got scalar value."),
        };

        let mut errors = ParameterError::new();

        let additional_properties = self.additional_properties();

        for (key, value) in map {
            if let Some((_optional, prop_schema)) = self.lookup(key) {
                if let Err(err) = prop_schema.verify_json(value) {
                    errors.add_errors(key, err);
                };
            } else if !additional_properties {
                errors.push(
                    key.to_string(),
                    format_err!("schema does not allow additional properties."),
                );
            }
        }

        for (name, optional, _prop_schema) in self.properties() {
            if !(*optional) && data[name] == Value::Null {
                errors.push(
                    name.to_string(),
                    format_err!("property is missing and it is not optional."),
                );
            }
        }

        if !errors.is_empty() {
            Err(errors.into())
        } else {
            Ok(())
        }
    }
}

impl ObjectSchemaType for ObjectSchema {
    fn description(&self) -> &'static str {
        self.description
    }

    fn lookup(&self, key: &str) -> Option<(bool, &Schema)> {
        ObjectSchema::lookup(self, key)
    }

    fn properties(&self) -> ObjectPropertyIterator {
        ObjectPropertyIterator {
            schemas: [].iter(),
            properties: Some(self.properties.iter()),
            nested: None,
        }
    }

    fn additional_properties(&self) -> bool {
        self.additional_properties
    }
}

impl ObjectSchemaType for AllOfSchema {
    fn description(&self) -> &'static str {
        self.description
    }

    fn lookup(&self, key: &str) -> Option<(bool, &Schema)> {
        AllOfSchema::lookup(self, key)
    }

    fn properties(&self) -> ObjectPropertyIterator {
        ObjectPropertyIterator {
            schemas: self.list.iter(),
            properties: None,
            nested: None,
        }
    }

    fn additional_properties(&self) -> bool {
        true
    }
}

#[doc(hidden)]
pub struct ObjectPropertyIterator {
    schemas: std::slice::Iter<'static, &'static Schema>,
    properties: Option<std::slice::Iter<'static, SchemaPropertyEntry>>,
    nested: Option<Box<ObjectPropertyIterator>>,
}

impl Iterator for ObjectPropertyIterator {
    type Item = &'static SchemaPropertyEntry;

    fn next(&mut self) -> Option<&'static SchemaPropertyEntry> {
        loop {
            match self.nested.as_mut().and_then(Iterator::next) {
                Some(item) => return Some(item),
                None => self.nested = None,
            }

            match self.properties.as_mut().and_then(Iterator::next) {
                Some(item) => return Some(item),
                None => match self.schemas.next()? {
                    Schema::AllOf(o) => self.nested = Some(Box::new(o.properties())),
                    Schema::Object(o) => self.properties = Some(o.properties.iter()),
                    _ => {
                        self.properties = None;
                        continue;
                    }
                },
            }
        }
    }
}

/// Schemas are used to describe complex data types.
///
/// All schema types implement constant builder methods, and a final
/// `schema()` method to convert them into a `Schema`.
///
/// ```
/// use proxmox_schema::{Schema, BooleanSchema, IntegerSchema, ObjectSchema};
///
/// const SIMPLE_OBJECT: Schema = ObjectSchema::new(
///     "A very simple object with 2 properties",
///     &[ // this arrays needs to be storted by name!
///         (
///             "property_one",
///             false /* required */,
///             &IntegerSchema::new("A required integer property.")
///                 .minimum(0)
///                 .maximum(100)
///                 .schema()
///         ),
///         (
///             "property_two",
///             true /* optional */,
///             &BooleanSchema::new("An optional boolean property.")
///                 .default(true)
///                 .schema()
///         ),
///     ],
/// ).schema();
/// ```
#[derive(Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub enum Schema {
    Null,
    Boolean(BooleanSchema),
    Integer(IntegerSchema),
    Number(NumberSchema),
    String(StringSchema),
    Object(ObjectSchema),
    Array(ArraySchema),
    AllOf(AllOfSchema),
}

impl Schema {
    /// Verify JSON value with `schema`.
    pub fn verify_json(&self, data: &Value) -> Result<(), Error> {
        match self {
            Schema::Null => {
                if !data.is_null() {
                    bail!("Expected Null, but value is not Null.");
                }
            }
            Schema::Object(s) => s.verify_json(data)?,
            Schema::Array(s) => s.verify_json(data)?,
            Schema::Boolean(s) => s.verify_json(data)?,
            Schema::Integer(s) => s.verify_json(data)?,
            Schema::Number(s) => s.verify_json(data)?,
            Schema::String(s) => s.verify_json(data)?,
            Schema::AllOf(s) => s.verify_json(data)?,
        }
        Ok(())
    }

    /// Parse a simple value (no arrays and no objects)
    pub fn parse_simple_value(&self, value_str: &str) -> Result<Value, Error> {
        let value = match self {
            Schema::Null => {
                bail!("internal error - found Null schema.");
            }
            Schema::Boolean(_boolean_schema) => {
                let res = parse_boolean(value_str)?;
                Value::Bool(res)
            }
            Schema::Integer(integer_schema) => {
                let res: isize = value_str.parse()?;
                integer_schema.check_constraints(res)?;
                Value::Number(res.into())
            }
            Schema::Number(number_schema) => {
                let res: f64 = value_str.parse()?;
                number_schema.check_constraints(res)?;
                Value::Number(serde_json::Number::from_f64(res).unwrap())
            }
            Schema::String(string_schema) => {
                string_schema.check_constraints(value_str)?;
                Value::String(value_str.into())
            }
            _ => bail!("unable to parse complex (sub) objects."),
        };
        Ok(value)
    }

    /// Parse a complex property string (`ApiStringFormat::PropertyString`)
    pub fn parse_property_string(&'static self, value_str: &str) -> Result<Value, Error> {
        // helper for object/allof schemas:
        fn parse_object<T: Into<ParameterSchema>>(
            value_str: &str,
            schema: T,
            default_key: Option<&'static str>,
        ) -> Result<Value, Error> {
            let mut param_list: Vec<(String, String)> = vec![];
            let key_val_list: Vec<&str> = value_str
                .split(|c: char| c == ',' || c == ';')
                .filter(|s| !s.is_empty())
                .collect();
            for key_val in key_val_list {
                let kv: Vec<&str> = key_val.splitn(2, '=').collect();
                if kv.len() == 2 {
                    param_list.push((kv[0].trim().into(), kv[1].trim().into()));
                } else if let Some(key) = default_key {
                    param_list.push((key.into(), kv[0].trim().into()));
                } else {
                    bail!("Value without key, but schema does not define a default key.");
                }
            }

            schema.into().parse_parameter_strings(&param_list, true).map_err(Error::from)
        }

        match self {
            Schema::Object(object_schema) => {
                parse_object(value_str, object_schema, object_schema.default_key)
            }
            Schema::AllOf(all_of_schema) => parse_object(value_str, all_of_schema, None),
            Schema::Array(array_schema) => {
                let mut array: Vec<Value> = vec![];
                let list: Vec<&str> = value_str
                    .split(|c: char| c == ',' || c == ';' || char::is_ascii_whitespace(&c))
                    .filter(|s| !s.is_empty())
                    .collect();

                for value in list {
                    match array_schema.items.parse_simple_value(value.trim()) {
                        Ok(res) => array.push(res),
                        Err(err) => bail!("unable to parse array element: {}", err),
                    }
                }
                array_schema.check_length(array.len())?;

                Ok(array.into())
            }
            _ => bail!("Got unexpected schema type."),
        }
    }
}

/// A string enum entry. An enum entry must have a value and a description.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct EnumEntry {
    pub value: &'static str,
    pub description: &'static str,
}

impl EnumEntry {
    /// Convenience method as long as we only have 2 mandatory fields in an `EnumEntry`.
    pub const fn new(value: &'static str, description: &'static str) -> Self {
        Self { value, description }
    }
}

/// String microformat definitions.
///
/// Strings are probably the most flexible data type, and there are
/// several ways to define their content.
///
/// ## Enumerations
///
/// Simple list all possible values.
///
/// ```
/// use proxmox_schema::{ApiStringFormat, EnumEntry};
///
/// const format: ApiStringFormat = ApiStringFormat::Enum(&[
///     EnumEntry::new("vm", "A guest VM run via qemu"),
///     EnumEntry::new("ct", "A guest container run via lxc"),
/// ]);
/// ```
///
/// ## Regular Expressions
///
/// Use a regular expression to describe valid strings.
///
/// ```
/// use proxmox_schema::{const_regex, ApiStringFormat};
///
/// const_regex! {
///     pub SHA256_HEX_REGEX = r"^[a-f0-9]{64}$";
/// }
/// const format: ApiStringFormat = ApiStringFormat::Pattern(&SHA256_HEX_REGEX);
/// ```
///
/// ## Property Strings
///
/// Use a schema to describe complex types encoded as string.
///
/// Arrays are parsed as comma separated lists, i.e: `"1,2,3"`. The
/// list may be sparated by comma, semicolon or whitespace.
///
/// Objects are parsed as comma (or semicolon) separated `key=value` pairs, i.e:
/// `"prop1=2,prop2=test"`. Any whitespace is trimmed from key and value.
///
///
/// **Note:** This only works for types which does not allow using the
/// comma, semicolon or whitespace separator inside the value,
/// i.e. this only works for arrays of simple data types, and objects
/// with simple properties (no nesting).
///
/// ```
/// use proxmox_schema::{ApiStringFormat, ArraySchema, IntegerSchema, Schema, StringSchema};
/// use proxmox_schema::{parse_simple_value, parse_property_string};
///
/// const PRODUCT_LIST_SCHEMA: Schema =
///             ArraySchema::new("Product List.", &IntegerSchema::new("Product ID").schema())
///                 .min_length(1)
///                 .schema();
///
/// const SCHEMA: Schema = StringSchema::new("A list of Product IDs, comma separated.")
///     .format(&ApiStringFormat::PropertyString(&PRODUCT_LIST_SCHEMA))
///     .schema();
///
/// let res = parse_simple_value("", &SCHEMA);
/// assert!(res.is_err());
///
/// let res = parse_simple_value("1,2,3", &SCHEMA); // parse as String
/// assert!(res.is_ok());
///
/// let data = parse_property_string("1,2", &PRODUCT_LIST_SCHEMA); // parse as Array
/// assert!(data.is_ok());
/// ```
pub enum ApiStringFormat {
    /// Enumerate all valid strings
    Enum(&'static [EnumEntry]),
    /// Use a regular expression to describe valid strings.
    Pattern(&'static ConstRegexPattern),
    /// Use a schema to describe complex types encoded as string.
    PropertyString(&'static Schema),
    /// Use a verification function.
    VerifyFn(fn(&str) -> Result<(), Error>),
}

impl std::fmt::Debug for ApiStringFormat {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ApiStringFormat::VerifyFn(fnptr) => write!(f, "VerifyFn({:p}", fnptr),
            ApiStringFormat::Enum(variants) => write!(f, "Enum({:?}", variants),
            ApiStringFormat::Pattern(regex) => write!(f, "Pattern({:?}", regex),
            ApiStringFormat::PropertyString(schema) => write!(f, "PropertyString({:?}", schema),
        }
    }
}

#[cfg(feature = "test-harness")]
impl Eq for ApiStringFormat {}

#[cfg(feature = "test-harness")]
impl PartialEq for ApiStringFormat {
    fn eq(&self, rhs: &Self) -> bool {
        match (self, rhs) {
            (ApiStringFormat::Enum(l), ApiStringFormat::Enum(r)) => l == r,
            (ApiStringFormat::Pattern(l), ApiStringFormat::Pattern(r)) => l == r,
            (ApiStringFormat::PropertyString(l), ApiStringFormat::PropertyString(r)) => l == r,
            (ApiStringFormat::VerifyFn(l), ApiStringFormat::VerifyFn(r)) => std::ptr::eq(l, r),
            (_, _) => false,
        }
    }
}

/// Parameters are objects, but we have two types of object schemas, the regular one and the
/// `AllOf` schema.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub enum ParameterSchema {
    Object(&'static ObjectSchema),
    AllOf(&'static AllOfSchema),
}

impl ParameterSchema {
    /// Parse key/value pairs and verify with object schema
    ///
    /// - `test_required`: is set, checks if all required properties are
    ///   present.
    pub fn parse_parameter_strings(
        self,
        data: &[(String, String)],
        test_required: bool,
    ) -> Result<Value, ParameterError> {
        do_parse_parameter_strings(self, data, test_required)
    }
}

impl ObjectSchemaType for ParameterSchema {
    fn description(&self) -> &'static str {
        match self {
            ParameterSchema::Object(o) => o.description(),
            ParameterSchema::AllOf(o) => o.description(),
        }
    }

    fn lookup(&self, key: &str) -> Option<(bool, &Schema)> {
        match self {
            ParameterSchema::Object(o) => o.lookup(key),
            ParameterSchema::AllOf(o) => o.lookup(key),
        }
    }

    fn properties(&self) -> ObjectPropertyIterator {
        match self {
            ParameterSchema::Object(o) => o.properties(),
            ParameterSchema::AllOf(o) => o.properties(),
        }
    }

    fn additional_properties(&self) -> bool {
        match self {
            ParameterSchema::Object(o) => o.additional_properties(),
            ParameterSchema::AllOf(o) => o.additional_properties(),
        }
    }
}

impl From<&'static ObjectSchema> for ParameterSchema {
    fn from(schema: &'static ObjectSchema) -> Self {
        ParameterSchema::Object(schema)
    }
}

impl From<&'static AllOfSchema> for ParameterSchema {
    fn from(schema: &'static AllOfSchema) -> Self {
        ParameterSchema::AllOf(schema)
    }
}

/// Helper function to parse boolean values
///
/// - true:  `1 | on | yes | true`
/// - false: `0 | off | no | false`
pub fn parse_boolean(value_str: &str) -> Result<bool, Error> {
    match value_str.to_lowercase().as_str() {
        "1" | "on" | "yes" | "true" => Ok(true),
        "0" | "off" | "no" | "false" => Ok(false),
        _ => bail!("Unable to parse boolean option."),
    }
}

/// Parse a complex property string (`ApiStringFormat::PropertyString`)
#[deprecated(note = "this is now a method of Schema")]
pub fn parse_property_string(value_str: &str, schema: &'static Schema) -> Result<Value, Error> {
    schema.parse_property_string(value_str)
}

/// Parse a simple value (no arrays and no objects)
#[deprecated(note = "this is now a method of Schema")]
pub fn parse_simple_value(value_str: &str, schema: &Schema) -> Result<Value, Error> {
    schema.parse_simple_value(value_str)
}

/// Parse key/value pairs and verify with object schema
///
/// - `test_required`: is set, checks if all required properties are
///   present.
#[deprecated(note = "this is now a method of parameter schema types")]
pub fn parse_parameter_strings<T: Into<ParameterSchema>>(
    data: &[(String, String)],
    schema: T,
    test_required: bool,
) -> Result<Value, ParameterError> {
    do_parse_parameter_strings(schema.into(), data, test_required)
}

fn do_parse_parameter_strings(
    schema: ParameterSchema,
    data: &[(String, String)],
    test_required: bool,
) -> Result<Value, ParameterError> {
    let mut params = json!({});

    let mut errors = ParameterError::new();

    let additional_properties = schema.additional_properties();

    for (key, value) in data {
        if let Some((_optional, prop_schema)) = schema.lookup(key) {
            match prop_schema {
                Schema::Array(array_schema) => {
                    if params[key] == Value::Null {
                        params[key] = json!([]);
                    }
                    match params[key] {
                        Value::Array(ref mut array) => {
                            match array_schema.items.parse_simple_value(value) {
                                Ok(res) => array.push(res), // fixme: check_length??
                                Err(err) => errors.push(key.into(), err),
                            }
                        }
                        _ => {
                            errors.push(key.into(), format_err!("expected array - type missmatch"))
                        }
                    }
                }
                _ => match prop_schema.parse_simple_value(value) {
                    Ok(res) => {
                        if params[key] == Value::Null {
                            params[key] = res;
                        } else {
                            errors.push(key.into(), format_err!("duplicate parameter."));
                        }
                    }
                    Err(err) => errors.push(key.into(), err),
                },
            }
        } else if additional_properties {
            match params[key] {
                Value::Null => {
                    params[key] = Value::String(value.to_owned());
                }
                Value::String(ref old) => {
                    params[key] = Value::Array(vec![
                        Value::String(old.to_owned()),
                        Value::String(value.to_owned()),
                    ]);
                }
                Value::Array(ref mut array) => {
                    array.push(Value::String(value.to_string()));
                }
                _ => errors.push(key.into(), format_err!("expected array - type missmatch")),
            }
        } else {
            errors.push(
                key.into(),
                format_err!("schema does not allow additional properties."),
            );
        }
    }

    if test_required && errors.is_empty() {
        for (name, optional, _prop_schema) in schema.properties() {
            if !(*optional) && params[name] == Value::Null {
                errors.push(
                    name.to_string(),
                    format_err!("parameter is missing and it is not optional."),
                );
            }
        }
    }

    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok(params)
    }
}

/// Verify JSON value with `schema`.
#[deprecated(note = "use the method schema.verify_json() instead")]
pub fn verify_json(data: &Value, schema: &Schema) -> Result<(), Error> {
    schema.verify_json(data)
}

/// Verify JSON value using a `StringSchema`.
#[deprecated(note = "use the method string_schema.verify_json() instead")]
pub fn verify_json_string(data: &Value, schema: &StringSchema) -> Result<(), Error> {
    schema.verify_json(data)
}

/// Verify JSON value using a `BooleanSchema`.
#[deprecated(note = "use the method boolean_schema.verify_json() instead")]
pub fn verify_json_boolean(data: &Value, schema: &BooleanSchema) -> Result<(), Error> {
    schema.verify_json(data)
}

/// Verify JSON value using an `IntegerSchema`.
#[deprecated(note = "use the method integer_schema.verify_json() instead")]
pub fn verify_json_integer(data: &Value, schema: &IntegerSchema) -> Result<(), Error> {
    schema.verify_json(data)
}

/// Verify JSON value using an `NumberSchema`.
#[deprecated(note = "use the method number_schema.verify_json() instead")]
pub fn verify_json_number(data: &Value, schema: &NumberSchema) -> Result<(), Error> {
    schema.verify_json(data)
}

/// Verify JSON value using an `ArraySchema`.
#[deprecated(note = "use the method array_schema.verify_json() instead")]
pub fn verify_json_array(data: &Value, schema: &ArraySchema) -> Result<(), Error> {
    schema.verify_json(data)
}

/// Verify JSON value using an `ObjectSchema`.
#[deprecated(note = "use the verify_json() method via the ObjectSchemaType trait instead")]
pub fn verify_json_object(data: &Value, schema: &dyn ObjectSchemaType) -> Result<(), Error> {
    schema.verify_json(data)
}

/// API types should define an "updater type" via this trait in order to support derived "Updater"
/// structs more easily.
///
/// Most trivial types can simply use an `Option<Self>` as updater. For types which do not use the
/// `#[api]` macro, this will need to be explicitly created (or derived via
/// `#[derive(UpdaterType)]`.
pub trait UpdaterType: Sized {
    type Updater: Updater;
}

#[cfg(feature = "api-macro")]
pub use proxmox_api_macro::UpdaterType;

#[cfg(feature = "api-macro")]
#[doc(hidden)]
pub use proxmox_api_macro::Updater;

macro_rules! basic_updater_type {
    ($($ty:ty)*) => {
        $(
            impl UpdaterType for $ty {
                type Updater = Option<Self>;
            }
        )*
    };
}
basic_updater_type! { bool u8 u16 u32 u64 i8 i16 i32 i64 usize isize f32 f64 String char }

impl<T> UpdaterType for Option<T>
where
    T: UpdaterType,
{
    type Updater = T::Updater;
}

// this will replace the whole Vec
impl<T> UpdaterType for Vec<T> {
    type Updater = Option<Self>;
}

pub trait ApiType {
    const API_SCHEMA: Schema;
}

impl<T: ApiType> ApiType for Option<T> {
    const API_SCHEMA: Schema = T::API_SCHEMA;
}

/// A helper type for "Updater" structs. This trait is *not* implemented for an api "base" type
/// when deriving an `Updater` for it, though the generated *updater* type does implement this
/// trait!
///
/// This trait is mostly to figure out if an updater is empty (iow. it should not be applied).
/// In that, it is useful when a type which should have an updater also has optional fields which
/// should not be serialized. Instead of `#[serde(skip_serializing_if = "Option::is_none")]`, this
/// trait's `is_empty` needs to be used via `#[serde(skip_serializing_if = "Updater::is_empty")]`.
pub trait Updater {
    /// Check if the updater is "none" or "empty".
    fn is_empty(&self) -> bool;
}

impl<T> Updater for Vec<T> {
    fn is_empty(&self) -> bool {
        self.is_empty()
    }
}

impl<T> Updater for Option<T> {
    fn is_empty(&self) -> bool {
        self.is_none()
    }
}

#[cfg_attr(feature = "test-harness", derive(Eq, PartialEq))]
pub struct ReturnType {
    /// A return type may be optional, meaning the method may return null or some fixed data.
    ///
    /// If true, the return type in pseudo openapi terms would be `"oneOf": [ "null", "T" ]`.
    pub optional: bool,

    /// The method's return type.
    pub schema: &'static Schema,
}

impl std::fmt::Debug for ReturnType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.optional {
            write!(f, "optional {:?}", self.schema)
        } else {
            write!(f, "{:?}", self.schema)
        }
    }
}

impl ReturnType {
    pub const fn new(optional: bool, schema: &'static Schema) -> Self {
        Self { optional, schema }
    }
}
