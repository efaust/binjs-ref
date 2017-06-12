use ast::grammar::*;
use token::io::*;
use util::{ f64_of, type_of };

use std::cell::*;
use std::fmt::Debug;

use serde_json;
use serde_json::Value;
type Object = serde_json::Map<String, Value>;

#[derive(Debug)]
pub enum Error<E> {
    Mismatch { expected: String, got: String },
    NoSuchInterface(String),
    NoSuchRefinement(String),
    NoSuchEnum(String),
    NoSuchKind(String),
    MissingField(String),
    NoSuchLiteral { strings: Vec<String> },
    TokenWriterError(E),
}

impl<E> From<E> for Error<E> {
    fn from(value: E) -> Self {
        Error::TokenWriterError(value)
    }
}


pub struct Encoder<'a, B, Tree, E> where B: TokenWriter<Tree=Tree, Error=E>, E: Debug {
    grammar: &'a Syntax,
    builder: RefCell<B>,
}
impl<'a, B, Tree, E> Encoder<'a, B, Tree, E> where B: TokenWriter<Tree=Tree, Error=E>, E: Debug {
    pub fn new(syntax: &'a Syntax, builder: B) -> Self {
        Encoder {
            grammar: syntax,
            builder: RefCell::new(builder),
        }
    }

    /// Finalize the encoder, recover the builder.
    pub fn done(self) -> B {
        self.builder.into_inner()
    }

    /// Encode a JSON into a SerializeTree based on a grammar.
    /// This step doesn't perform any interesting check on the JSON.
    pub fn encode(&self, value: &Value, kind: &Type) -> Result<Tree, Error<E>> {
        debug!("encode: value {:?} with kind {:?}", value, kind);
        use ast::grammar::Type::*;
        match *kind {
            Array(ref kind) => {
                let list = value.as_array()
                    .ok_or_else(|| Error::Mismatch {
                        expected: "Array".to_string(),
                        got: type_of(&value)
                    })?;
                let mut encoded = Vec::with_capacity(list.len());
                for item in list {
                    let item = self.encode(item, kind)?;
                    encoded.push(item);
                }
                let result = self.builder.borrow_mut().list(encoded)
                    .map_err(Error::TokenWriterError)?;
                Ok(result)
            }
            Obj(ref structure) => {
                let object = value.as_object()
                    .ok_or_else(|| Error::Mismatch {
                        expected: "Object".to_string(),
                        got: type_of(&value)
                    })?;
                let mut contents = self.encode_structure(object, structure.fields())?;
                let contents : Vec<_> = contents
                    .drain(..)
                    .map(|(_, tree)| tree)
                    .collect();
                // This is an anonymous structure, so we expect that the order
                // of fields has been specified elsewhere.
                let result = self.builder.borrow_mut().untagged_tuple(&contents)
                    .map_err(Error::TokenWriterError)?;
                Ok(result)
            }
            Enum(ref name) => {
                let enum_ = self.grammar.get_enum_by_name(&name)
                    .ok_or_else(|| Error::NoSuchEnum(name.to_string().clone()))?;
                let string = value.as_str()
                    .ok_or_else(|| Error::Mismatch {
                        expected: "String".to_string(),
                        got: type_of(&value)
                    })?;
                for candidate in enum_.strings() {
                    if candidate == string {
                        let result = self.builder.borrow_mut().string(Some(&candidate))
                            .map_err(Error::TokenWriterError)?;
                        return Ok(result)
                    }
                }
                Err(Error::NoSuchLiteral {
                    strings: enum_.strings().iter().cloned().collect(),
                })
           }
           // Special-case: hardcoded `"Null"`.
           Interfaces {
               or_null: true,
               ..
           } if value.is_null() => {
               self.builder
                   .borrow_mut()
                   .tagged_tuple(&"Null", &[])
                   .map_err(Error::TokenWriterError)
           }
           Interfaces {
               names: ref interfaces,
               ..
           } => {
               let object = value.as_object()
                   .ok_or_else(|| Error::Mismatch {
                       expected: "Object (implementing interface)".to_string(),
                       got: type_of(&value)
                   })?;
               let type_field = object.get("type")
                   .ok_or_else(|| Error::MissingField("type".to_string()))?;
               let kind_name = type_field
                   .as_str()
                   .ok_or_else(|| Error::Mismatch {
                       expected: "type name (as String)".to_string(),
                       got: type_of(type_field)
                   })?;
               let kind = self.grammar.get_kind(kind_name)
                   .ok_or_else(|| Error::NoSuchKind(kind_name.to_string().clone()))?;

               // We have a kind, so we know how to encode the data. We just need
               // to make sure that we expected this interface here.
               // FIXME: Is this really necessary?
               if let Some(interface) = self.grammar.get_interface_by_kind(&kind) {
                   if self.grammar
                        .get_ancestors_by_name_including_self(interface.name())
                        .unwrap()
                        .iter()
                        .find(|ancestor|
                            interfaces.iter()
                                .find(|candidate| {
                                    debug!("Looking for {:?} =?= {:?}", candidate, ancestor);
                                    candidate == ancestor
                                })
                                .is_some()
                        ).is_none() {
                        return Err(Error::NoSuchRefinement(kind.to_string().clone()))
                   }
                   let fields = interface.contents().fields();
                   let contents = self.encode_structure(object, fields)?;
                   // Write the contents with the tag of the refined interface.
                   let labelled = self.builder
                       .borrow_mut()
                       .tagged_tuple(&kind.to_string(), &contents)
                       .map_err(Error::TokenWriterError)?;
                   return Ok(labelled)
               }
               return Err(Error::NoSuchRefinement(kind.to_string().clone()));
           }
           Boolean => {
                let value = value.as_bool()
                    .ok_or_else(|| Error::Mismatch {
                        expected: "bool".to_string(),
                        got: type_of(&value)
                    })?;
                let result = self.builder.borrow_mut().bool(value)
                    .map_err(Error::TokenWriterError)?;
                Ok(result)
           }
           String => {
                let value = value.as_str()
                    .ok_or_else(|| Error::Mismatch {
                        expected: "String".to_string(),
                        got: type_of(&value)
                    })?
                   .to_owned();
                let result = self.builder.borrow_mut().string(Some(&value))
                    .map_err(Error::TokenWriterError)?;
                Ok(result)
           }
           Number => {
                let value =
                    if let serde_json::Value::Number(ref num) = *value {
                        f64_of(num)
                    } else {
                        return Err(Error::Mismatch {
                            expected: "Number".to_string(),
                            got: type_of(&value)
                        })
                    };
                let result = self.builder.borrow_mut().float(value)
                   .map_err(Error::TokenWriterError)?;
                Ok(result)
           }
        }
    }

    fn encode_structure<'b>(&self, object: &'b Object, fields: &'b [Field]) -> Result<Vec<(&'b Field, B::Tree)>, Error<E>> {
        let mut result = Vec::with_capacity(fields.len());
        for field in fields {
            if let Some(source) = object.get(field.name().to_string()) {
                let encoded = self.encode(source, field.type_())?;
                result.push((field, encoded))
            } else {
                return Err(Error::MissingField(field.name().to_string().clone()))
            }
        }
        Ok(result)
    }
}