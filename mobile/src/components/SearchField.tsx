import type { ComponentRef, Ref } from "react";
import type { StyleProp, TextInput, TextInputProps, ViewStyle } from "react-native";
import { FieldInput } from "./FieldInput";
import { Icon } from "./Icon";

export interface SearchFieldProps extends Omit<TextInputProps, "style"> {
  label: string;
  /** Layout styling belongs to the entire search control, including its icon. */
  style?: StyleProp<ViewStyle>;
  inputStyle?: TextInputProps["style"];
  ref?: Ref<ComponentRef<typeof TextInput>>;
}

export function SearchField({ label, placeholder = label, style, inputStyle, ...props }: SearchFieldProps) {
  return (
    <FieldInput
      autoCapitalize="none"
      autoCorrect={false}
      returnKeyType="search"
      accessibilityRole="search"
      {...props}
      containerStyle={style}
      style={inputStyle}
      accessibilityLabel={label}
      placeholder={placeholder}
      leading={<Icon name="search" size={16} tone="textSubtle" />}
    />
  );
}
